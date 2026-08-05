# Vakt OS

A Linux security appliance built from scratch — custom init, package manager,
TUI, service supervisor, and framebuffer compositor, written in Rust and Go.

There is no systemd, no glibc userland, and no distro underneath it. The image
is a statically-linked busybox rootfs, a Rust program as PID 1, and the tools
in this repository.

```
GRUB → vmlinuz → initramfs → /init (vakt-init, Rust)
                                │
                                ├── mounts /proc /sys /dev, /dev/sda → /persistent
                                ├── supervises background services
                                │     ├── vakt-net   (Wi-Fi + DHCP)
                                │     └── vakt-ids   (filesystem integrity)
                                └── runs vakt-panel (TUI) on the console
                                          └── vakt-compositor (raw /dev/fb0)
```

## Components

| Component | Language | Role |
|---|---|---|
| `vakt-init` | Rust | PID 1. Mounts filesystems, mounts the persistent disk, supervises services, runs the panel. |
| `vakt-net` | Rust | Brings up networking asynchronously so boot never blocks on a radio. |
| `vakt-ids` | Go | Host intrusion detection: SHA-256 baseline of watched paths, reports tampering. |
| `vakt-panel` | Go | tview TUI — the appliance's primary interface. |
| `vakt-audit` | Go | CIS-style compliance checks. |
| `vakt-compositor` | Rust | Draws directly to `/dev/fb0` via mmap. No X11, no Wayland. |
| `zrpkg` | Rust | Package manager: fetch, verify ed25519 signature, unpack. |
| `zrpkg-server` | Go | Host-side HTTP repository server. |

## Building

Requires an Arch host (the build script uses `pacman`), root, and a kernel at
`/boot/vmlinuz-linux` — the ISO reuses the host kernel and its modules.

```bash
sudo ./build.sh
```

This compiles everything, builds and signs the package repository, assembles
the rootfs, and produces `vakt-os.iso` plus a 256MB `vakt-data.img`.

The data disk is **not** recreated if it already exists, because it holds your
Wi-Fi credentials, installed packages, and the IDS baseline. Delete it by hand
to start clean.

## Running

Serve the package repository from the host first:

```bash
./tools/bin/zrpkg-server -dir tools/repo
```

Then boot. The data disk must be the first drive so it lands on `/dev/sda`,
and QEMU user networking puts the host at `10.0.2.2`:

```bash
qemu-system-x86_64 -m 2G \
    -drive file=vakt-data.img,format=raw,index=0,media=disk \
    -cdrom vakt-os.iso \
    -netdev user,id=n0 -device e1000,netdev=n0
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

## Packages

`zrpkg` fetches `<name>.zrp` and `<name>.json` over HTTP, verifies the archive
against an ed25519 signature, and unpacks it into `$ZRPKG_ROOT`
(`/persistent/zrpkg`, so installs survive reboots). `vakt-init` puts
`$ZRPKG_ROOT/usr/bin` on `PATH`.

```
zrpkg update              # list what the repository offers
zrpkg install vakt-audit  # fetch, verify, install
```

Build the repository on the host with:

```bash
./build-system/mkrepo.sh
```

It generates a signing key on first run at `build-system/keys/repo.key`,
signs each package, and writes `tools/repo/`. `build.sh` copies the matching
public key into the image as `/etc/vakt/trusted.key`.

> **The private key is gitignored and is not in this repository.** A fresh
> clone generates its own on the first `mkrepo.sh` run. If no trust anchor is
> present, `zrpkg` warns loudly and installs unverified; if one is present, a
> package failing verification is deleted rather than unpacked.

## Services

`vakt-init` is a small service supervisor. Each service gets a PID file at
`/run/<name>.pid` and its output captured to `/run/<name>.log`; the supervisor
restarts daemons that die and gives up on any that crash more than five times
in sixty seconds, so a broken binary can never become a spin loop. A summary
is written to `/run/services.status` for the panel's Services page.

It reaps children with a per-PID `try_wait()` rather than a wildcard
`waitpid(-1)`. As PID 1 it could reap everything, but the main thread collects
`vakt-panel`'s exit status with `Command::status()`, and a wildcard reaper
would race that call and swallow the result.

## Layout

```
build.sh                  Full ISO build
build-system/mkrepo.sh    Builds and signs the package repository
build-system/             Kernel config, bootstrap, logo
pkg-manager/              zrpkg (Rust)
vakt-init/                PID 1 and the service supervisor (Rust)
vakt-net/                 Networking daemon (Rust)
vakt-compositor/          Framebuffer compositor (Rust)
tools/cmd/                Go tools: panel, audit, ids, repo server
```

## Tests

```bash
cargo test --manifest-path vakt-init/Cargo.toml   # supervisor behaviour
cargo test --manifest-path vakt-net/Cargo.toml    # config parsing
cd tools && go test ./cmd/...                     # panel status rendering
```

The supervisor tests cover crash-loop abandonment, unstartable binaries, log
capture, and the guarantee that background reaping does not steal a foreground
child's exit code.
