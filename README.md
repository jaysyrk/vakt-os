# Vakt OS

[Readme](README.md) | [Roadmap](ROADMAP.md) | [Contributing](CONTRIBUTING.md)

A Linux security appliance built from scratch — custom init, package manager,
TUI, service supervisor, and framebuffer compositor, written in Rust, Go, and
Zig.

There is no systemd, no glibc userland, and no distro underneath it. The image
is a statically-linked busybox rootfs, a Rust program as PID 1, and the tools
in this repository.

```
GRUB → vmlinuz → initramfs → /init (vakt-init, Rust, PID 1)
                                │
                                ├── mounts /proc /sys /dev, tmpfs over /run and /tmp
                                ├── mounts /dev/sda → /persistent
                                ├── remounts / read-only
                                ├── supervises background services
                                │     ├── vakt-net   (Wi-Fi + DHCP, Landlock sandboxed)
                                │     └── vakt-ids   (filesystem integrity)
                                ├── waits for them on /run/init.sock
                                └── drops to uid 1000 and runs vakt-panel (TUI)
                                          └── vakt-compositor (raw /dev/fb0, Landlock sandboxed)
```

## Components

| Component | Language | Role |
|---|---|---|
| `vakt-init` | Rust | PID 1. Mounts filesystems, seals the root, supervises services, drops privileges, shuts the system down. |
| `vakt-net` | Rust | Brings up networking asynchronously so boot never blocks on a radio. |
| `vakt-ids` | Go | Host intrusion detection: SHA-256 baseline of watched paths, reports tampering. |
| `vakt-panel` | Go | tview TUI — the appliance's primary interface. Runs unprivileged. |
| `vakt-audit` | Go | CIS-style compliance checks. |
| `vakt-compositor` | Rust | Draws directly to `/dev/fb0` via mmap. No X11, no Wayland. |
| `zrpkg` | Rust | Package manager: resolve dependencies, fetch, verify ed25519 signature, unpack, uninstall. |
| `zrpkg-server` | Go | Host-side HTTP repository server. |
| `vakt-verify` | Zig | Independent, from-scratch re-check of a package signature — no code shared with `zrpkg`. |

## Security model

Four things hold the system together. Each of them is enforced by the kernel
rather than by convention, which is the only kind of enforcement worth having.

**The root filesystem is read-only.** The initramfs is unpacked into RAM, and
once `vakt-init` has finished with it there is nothing left to write, so it is
remounted `ro`. Everything writable is mounted over the top: `/run` and `/tmp`
are `nosuid,nodev,noexec` tmpfs with size caps, and `/persistent` is the data
disk, mounted `nosuid,nodev`. `/etc/resolv.conf` and `/etc/mtab` are symlinks
into `/run` and `/proc`, because DHCP and `mount` expect to write them.

**The panel is not root.** `vakt-init` stays root, but it launches the panel
through `setgroups`/`setgid`/`setuid` to uid 1000 (`vakt`), verifying afterwards
that uid 0 is genuinely unreachable. The console devices and the writable paths
the panel needs — its home on the tmpfs, the package install root, the directory
holding the network configuration — are handed over explicitly and nothing else
is. The package install root is deliberately *not* on PID 1's `PATH`: it is
writable by an unprivileged user, so a binary planted there must never be a
candidate for something root would run.

Booting the *Vakt OS (root recovery shell)* GRUB entry adds `vakt.rootshell` to
the kernel command line, which gives the fallback shell root when the panel
exits. That is for repairing an image whose panel will not start.

**Two daemons confine themselves with Landlock.** `vakt-compositor` restricts
itself to `/dev/fb0` and nothing else — the ruleset is applied before the device
is opened, so if it were wrong the compositor would fail immediately rather than
quietly run unconfined. `vakt-net` keeps read and execute on the image's program
directories (it drives `ip`, `wpa_supplicant` and `udhcpc`, and the ruleset is
inherited by all of them), read and write on `/run`, and read on exactly one
path under `/persistent`: its own configuration file. The rest of the data disk
— installed packages, the IDS baseline — is unreachable from the daemon that
talks to the network. Landlock is applied best-effort, so a kernel without it
degrades to unsandboxed and says so in the log; the supplied kernel
configuration builds it in and the GRUB entry puts it in the LSM stack.

**Packages must be signed.** There is no unverified install path. If the image
has no trust anchor, or the archive does not verify against it, `zrpkg` deletes
the download and stops. See [Packages](#packages).

**Signatures get a second, independent opinion.** `vakt-verify` re-implements
the same SHA-256 + Ed25519 check in Zig, against only its standard library —
no code, no crate, no line shared with `zrpkg`'s Rust verifier. It runs
automatically in `build-system/mkrepo.sh`, checking every package the moment
after `zrpkg` signs it, and refuses to publish the repository if the two
disagree. A bug or a backdoor in one verifier is not automatically a bug in
the other.

## Building

Requires an Arch host (the build script uses `pacman`) and root.

```bash
sudo ./build.sh
```

This compiles everything, builds and signs the package repository, assembles
the rootfs, builds a kernel, and produces `vakt-os.iso` plus a 256MB
`vakt-data.img`.

Two kernel modes:

| | `VAKT_KERNEL=host` (default) | `VAKT_KERNEL=custom` |
|---|---|---|
| Kernel | `/boot/vmlinuz-linux` from the build machine | Built from `build-system/kernel.config` |
| Modules | The host's `/lib/modules` and `/lib/firmware` | None — monolithic |
| Size | Large; the firmware tree alone is hundreds of MB | Small |
| Hardware | Whatever the host kernel supports | QEMU, common wired NICs, NVMe/AHCI, USB HID/storage — no Wi-Fi |
| Build time | Seconds | Long the first time; the source tree is cached |

**`host` is the one for real hardware you didn't hand-pick drivers for in
advance.** Build it *on* the machine you're installing to, or one of the same
generation, so the kernel's modules actually match the hardware:

```bash
sudo ./build.sh
```

`custom` trades that coverage for a small, from-scratch, no-modules image —
right for a VM or a specific known machine, not for "boot this on whatever PC
I have":

```bash
sudo VAKT_KERNEL=custom ./build.sh
```

To ship an image that already knows where its packages live, name the
repository at build time:

```bash
sudo VAKT_REPO_URL=https://packages.example.com ./build.sh
```

The data disk is **not** recreated if it already exists, because it holds your
Wi-Fi credentials, installed packages, and the IDS baseline. Delete it by hand
to start clean.

## Running

### In a VM

Serve the package repository from the host first:

```bash
./tools/bin/zrpkg-server -dir tools/repo
```

(Or point the appliance at a repository on a server you rent, and skip this —
see [Remote repositories](#remote-repositories).)

Then boot. The data disk must be the first drive so it lands on `/dev/sda`,
and QEMU user networking puts the host at `10.0.2.2`:

```bash
qemu-system-x86_64 -m 2G -enable-kvm \
    -drive file=vakt-data.img,format=raw,index=0,media=disk \
    -cdrom vakt-os.iso \
    -netdev user,id=n0 -device e1000,netdev=n0
```

### On real hardware

```bash
lsblk                       # find the USB drive - not your main disk
sudo dd if=vakt-os.iso of=/dev/sdX bs=4M status=progress conv=fsync
```

Persistent storage works the same way it does in QEMU — the kernel needs a
disk it enumerates as `/dev/sda` — but there is no `index=0` to force it on
real hardware. Use a **second** physical drive or USB stick, formatted ext4,
and check `dmesg` after boot to confirm it actually landed on `sda`; if it
did not, `vakt-init` just runs in RAM-only mode instead of failing, so nothing
breaks, but nothing persists either.

```bash
sudo mkfs.ext4 /dev/sdY     # the persistent disk, separate from the boot USB
```

**Disable Secure Boot.** This GRUB is not signed for it, and it will refuse
to load. Pick the plain "Vakt OS" entry in the boot menu; the "root recovery
shell" entry is for when the panel will not start.

There is no QEMU gateway on real hardware, so point `zrpkg` at a real
repository rather than the default (see
[Remote repositories](#remote-repositories)) if you want to install anything.

## The kernel

`build-system/kernel.config` is a seed, not a finished `.config`.
`build-system/mkkernel.sh` feeds it to `make allnoconfig KCONFIG_ALLCONFIG=…`,
which starts every symbol in the tree at "no" and enables only what the seed
asks for plus its dependencies. Nothing is stripped, because nothing
unasked-for was ever turned on.

`CONFIG_MODULES` is off. Every driver is built in, there is no module loader to
attack, and the image has no `/lib/modules` or `/lib/firmware` to ship — which
is most of the reason the ISO is small.

The seed is split at an `OPTIONAL` marker. Everything above it is checked
against the generated `.config` afterwards and the build **fails** listing
anything that did not survive, because a symbol silently dropped for a missing
dependency is an unbootable image and should not be discovered in QEMU.
Everything below it is exploit mitigations, whose names drift between releases
(the Spectre options became `CONFIG_MITIGATION_*` in 6.8); those are applied
where they exist and reported where they do not.

```bash
KERNEL_VERSION=6.12.41 KERNEL_SHA256=<sha> ./build-system/mkkernel.sh
```

## Networking

Boot does not prompt for anything. `vakt-net` runs in the background and:

- with **no configuration**, requests DHCP on `eth0` — which is what lets a
  fresh VM reach the package repository immediately under QEMU;
- with a configuration at `/persistent/etc/vakt-net.conf`, associates with the
  named Wi-Fi network and then requests a lease.

Write that file from the panel's **Wi-Fi Setup** page, or by hand:

```ini
ssid=MyNetwork
psk=hunter2
interface=wlan0
```

The daemon polls the file's modification time, so saving it triggers a
reconnect with no restart needed. Current state is published to
`/run/vakt-net.status` and shown on the panel's Network page.

**Wi-Fi needs `VAKT_KERNEL=host`.** The `custom` kernel builds in the 802.11
stack (`cfg80211`, `mac80211`) but no chipset driver: real Wi-Fi hardware
needs a proprietary firmware blob loaded at runtime, and carrying those is
exactly what the monolithic, no-`/lib/firmware` build exists not to do. If you
need Wi-Fi, build with the host kernel instead — it brings the build
machine's own modules and firmware tree with it.

`custom` mode's wired coverage is narrower than `host` too, though broader
than just QEMU: Intel e1000/e1000e, Realtek r8169, and Broadcom tg3, plus
virtio-net. r8169 in particular enumerates most Realtek revisions without a
firmware file, but some of the newest chip generations will show up in
`dmesg` and still refuse to link — another point in favor of `host` mode on
hardware you have not already tested.

## Packages

`zrpkg` fetches `<name>.zrp` and `<name>.json` over HTTP, verifies the archive
against an ed25519 signature, and unpacks it into `$ZRPKG_ROOT`
(`/persistent/zrpkg`, so installs survive reboots).

```
zrpkg update              # list what the repository offers
zrpkg install vakt-audit  # resolve, fetch, verify, install
zrpkg verify vakt-audit   # check the signature without installing
zrpkg remove vakt-audit   # delete exactly what the install created
zrpkg repo                # show where packages come from
zrpkg repo <url>          # fetch from somewhere else from now on
```

**Trust is mandatory.** The trust anchor is `/etc/vakt/trusted.key`, put there
by `build.sh`. With no key, or with an archive that fails verification, the
download is deleted and the install stops — there is no warn-and-continue path.

**Dependencies are a graph.** A manifest may name the packages it needs; those
manifests name theirs. `zrpkg` fetches the reachable set, topologically sorts it
so nothing is unpacked before what it depends on, and reports a cycle by naming
the loop rather than recursing into it. Already-installed packages at the same
version are skipped.

**Removal is exact.** Installing records every path the archive produced in
`$ZRPKG_ROOT/var/lib/zrpkg/<name>.json`, and `zrpkg remove` works only from that
list — nothing is deleted for looking like it belongs to a package. Recorded
paths are re-validated before they are touched, so a crafted archive cannot
record `../../etc/passwd` at install time and have removal act on it later, and
deletion never follows a symlink. Removing a package another one still depends
on is refused unless you pass `--force`.

Build the repository on the host with:

```bash
./build-system/mkrepo.sh
```

It generates a signing key on first run at `build-system/keys/repo.key`,
signs each package, writes `tools/repo/`, and records each package's
dependencies in the manifest and the index. `build.sh` copies the matching
public key into the image as `/etc/vakt/trusted.key`.

Every archive it signs is immediately re-checked by `vakt-verify`, so the
build fails before anything is published rather than after:

```
    Signature:  b908d6a1...
    Public key: 4d29b6f1...
PASS  tool  sha256:6eced72d...
```

You can run the same check yourself, against any package and any key, without
installing anything:

```bash
vakt-verify tools/repo/vakt-audit.zrp tools/repo/vakt-audit.json \
    --pubkey-file build-system/keys/repo.pub
```

> **The private key is gitignored and is not in this repository.** A fresh
> clone generates its own on the first `mkrepo.sh` run, which means an image
> built from a fresh clone will not install packages signed by anyone else's.

### Remote repositories

The `10.0.2.2` default is the QEMU host — right for developing on a laptop,
wrong for anything deployed. An appliance in the field fetches from a server,
and that has to be changeable without rebuilding the image.

Four places are consulted, most specific first:

| | |
|---|---|
| `ZRPKG_REPO_URL` | one-off overrides and scripts |
| `/persistent/etc/zrpkg.conf` | the data disk — what `zrpkg repo` and the panel write, survives reboots |
| `/etc/vakt/zrpkg.conf` | baked in by `VAKT_REPO_URL=…  ./build.sh` |
| built-in | the QEMU host |

From the panel's **Packages** page, or a shell:

```bash
zrpkg repo https://packages.example.com
zrpkg update
```

**The repository URL is not a trust decision.** Packages are verified against
`/etc/vakt/trusted.key` whatever server served them, so pointing this at a
hostile mirror gets you failed signature checks, not compromised packages. What
plain HTTP does leak is *which* packages you fetch, to anyone on the path —
which is why `zrpkg-server` can terminate TLS itself, and why the panel says so
when you set an `http://` URL.

`zrpkg-server` is built to be exposed: read-only methods, a flat namespace so
traversal has nowhere to resolve to, only `.zrp` and `.json` served, no
directory listings, bounded timeouts, and a `SIGTERM` that drains in-flight
downloads rather than truncating them.

**[`deploy/`](deploy/README.md)** has the rest: a hardened systemd unit, an
nginx reverse-proxy snippet, TLS both ways round, and `publish.sh`, which signs
locally and rsyncs only the signed output — the signing key never reaches the
server.

```bash
./deploy/publish.sh user@vps.example.com
```

## Services

`vakt-init` is a small service supervisor. Each service gets a PID file at
`/run/<name>.pid` and its output captured to `/run/<name>.log`; the supervisor
restarts daemons that die and gives up on any that crash more than five times
in sixty seconds, so a broken binary can never become a spin loop. A summary
is written to `/run/services.status` for the panel's Services page.

It reaps children with a per-PID `try_wait()` rather than a wildcard
`waitpid(-1)`. As PID 1 it could reap everything, but the main thread collects
`vakt-panel`'s exit status itself, and a wildcard reaper would race that call
and swallow the result.

### Readiness

`/run/init.sock` is a Unix datagram socket that daemons use to say they have
finished starting. Boot waits there instead of guessing a delay, so the panel is
drawn when the system is actually usable — and does not wait at all for a
service that has already failed to start.

The format is the same shape as systemd's `sd_notify`: newline-separated
`KEY=value` pairs in one datagram, which costs a client about fifteen lines and
no library.

```
READY=1
STATUS=10.0.2.15 on eth0
```

The sender is identified by the credentials the kernel attaches to the datagram
(`SO_PASSCRED`), not by anything in the message body, so one service cannot
report readiness on another's behalf. The socket is `root:vakt` mode 0660, which
is also how the unprivileged panel is able to send `SHUTDOWN=poweroff`.

### Log rotation

`/run` is a tmpfs, so every byte a daemon logs is a byte of RAM that does not
come back. Service output is piped through a writer with a 5 MB budget per
service: the active log rotates to `<name>.log.1` at half the budget and a fresh
one starts, so a daemon stuck printing an error cannot exhaust memory, and the
last thing it said before dying is still there.

### Shutdown

The kernel applies no default signal dispositions to PID 1, so a signal that
would end any other process is discarded by init unless it is handled. Signals
are blocked process-wide at startup and read off a `signalfd` on a dedicated
thread, which removes the async-signal-safety question rather than working
around it.

| Signal | Action | Sent by |
|---|---|---|
| `SIGTERM`, `SIGINT` | reboot | busybox `reboot`; ctrl-alt-del |
| `SIGPWR`, `SIGUSR2` | power off | busybox `poweroff` |
| `SIGUSR1` | halt | busybox `halt` |

Whatever the trigger — a signal, or `SHUTDOWN=` on the init socket from the
panel's Power page — the same sequence runs: end the console session, SIGTERM
every supervised daemon and wait (SIGKILL after five seconds), `sync`, unmount
`/persistent`, then `reboot(2)`. `vakt-init` also disables the kernel's own
ctrl-alt-del handling at boot, so that key combination goes through this
sequence instead of cutting power to a mounted disk.

## Layout

```
build.sh                    Full ISO build
build-system/mkkernel.sh    Builds the monolithic kernel
build-system/mkrepo.sh      Builds and signs the package repository
build-system/kernel.config  Kernel configuration seed
pkg-manager/                zrpkg (Rust)
vakt-init/                  PID 1, supervisor, readiness, shutdown (Rust)
vakt-net/                   Networking daemon (Rust)
vakt-compositor/            Framebuffer compositor (Rust)
vakt-verify/                Independent signature verifier (Zig)
tools/cmd/                  Go tools: panel, audit, ids, repo server
deploy/                     Running the repository on a server you rent
.github/workflows/build.yml CI: tests, package pipeline, ISO artifact
```

## Tests

```bash
cargo test --manifest-path vakt-init/Cargo.toml    # supervisor, logs, readiness, shutdown
cargo test --manifest-path pkg-manager/Cargo.toml  # graph, trust, install, removal
cargo test --manifest-path vakt-net/Cargo.toml     # config parsing, notification
cd tools && go test ./cmd/...                      # panel rendering, repository server
cd vakt-verify && zig build test                   # hex/arg parsing, signed-message pinning
```

CI runs all of these on `archlinux:latest`, then drives the package manager
end to end against a real repository server — pack and sign a three-package
dependency chain, install the top one, confirm the graph was walked in order,
confirm a tampered archive is refused, confirm removal is refused while
something still depends on the package, and confirm `vakt-verify` agrees with
every signature `zrpkg` produced and rejects a tampered one — then builds the
ISO and uploads it as an artifact. Tagging `v*` publishes it as a release
asset, because the image is well over what the repository will hold.

## Third-party components

Almost everything here is written from scratch; these are the pieces that are
not, and where they come from.

**Shipped in the image**

| Component | Source | Licence |
|---|---|---|
| busybox 1.35.0 (static, musl) | [busybox.net prebuilt binary](https://www.busybox.net/downloads/binaries/1.35.0-x86_64-linux-musl/busybox), pinned by SHA-256 in `build.sh` | GPL-2.0 |
| Linux kernel | [kernel.org](https://kernel.org), configured by `build-system/kernel.config` | GPL-2.0 |
| GRUB | Arch host package, used by `grub-mkrescue` | GPL-3.0 |
| wpa_supplicant, wpa_passphrase | Arch host package ([w1.fi](https://w1.fi/wpa_supplicant/)) | BSD-3-Clause |
| CA certificate bundle | Arch host `ca-certificates` | MPL-2.0 |

**Rust dependencies** — `nix` (syscalls), `landlock` (LSM sandboxing), `clap`,
`anyhow`, `tokio`, `reqwest`, `serde`/`serde_json`, `ed25519-dalek`, `sha2`,
`hex`, `tar`, `flate2`, `libc`, `memmap2`. Exact versions are in each crate's
`Cargo.toml` and `Cargo.lock`.

**Go dependencies** — [`rivo/tview`](https://github.com/rivo/tview) and
[`gdamore/tcell`](https://github.com/gdamore/tcell) for the panel TUI, and their
transitive dependencies. See `tools/go.mod`.

**Zig dependencies** — none. `vakt-verify` uses only `std.crypto` (SHA-256,
Ed25519), `std.json`, and `std.process`/`std.Io` from the standard library
that ships with the compiler itself.

**Designs borrowed rather than code**

- The readiness protocol on `/run/init.sock` reuses the wire format of
  systemd's [`sd_notify`](https://www.freedesktop.org/software/systemd/man/sd_notify.html).
  No systemd code or library is involved; the format is `KEY=value` lines and
  the implementations here are a few dozen lines in Rust and Go.
- The hardening options below the `OPTIONAL` marker in `build-system/kernel.config`
  follow the [Kernel Self Protection Project](https://kernsec.org/wiki/index.php/Kernel_Self_Protection_Project/Recommended_Settings)
  recommended settings.

**CI** — `actions/checkout`, `actions/cache`, `actions/upload-artifact`, and
[`softprops/action-gh-release`](https://github.com/softprops/action-gh-release).

## License

Vakt OS is licensed under the [Apache License, Version 2.0](LICENSE). In
short: you may use, modify, and redistribute this code, including
commercially, as long as you keep the copyright and license notices (see
[NOTICE](NOTICE)) and state what you changed — it may not be relicensed as
someone else's unattributed work. The third-party components listed above
keep their own licenses regardless of this project's license.
