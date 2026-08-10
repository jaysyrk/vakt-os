# Vakt OS

```
  .%.                     %=
  .%%%                  +%%=
  .%%%%.               %%%%=
  %.%%%%%.           %%%%%.%.
.%%%%.%%%%%.       %%%%%.+%%%.
@%%%. ..%%%%*    .%%%%:.  %%%%
%%%%.    *%%%%  %%%%%.    %%%%
 %%%%..  ..%%%%%%%%..   .%%%%.
 .%%%%%%%%%%.%%%%.%%%%%%%%%%.
   .+%%%%%%..%%%%..%%%%%%%.
  %.        .%%%%.        .%
 .%%%%%.     .%%.      *%%%%.
  %%%%%%%.     .     %%%%%%%.
  %%%%..@%%.       %%%:.%%%%
  .%%%.      .   .      %%%*
   %%%.                .%%%.
   .%%%.               %%%.
    .%%%              %%%.
     .%%%            %%%.
      .%%%.         %%%
        .%%.       %%.
          .%.    .%.
              . .
```

| [Roadmap](ROADMAP.md) |

**Claude was used solely for .md formats, .sh scripts, and git push automation**

**A small, immutable Linux security appliance: read-only root, signed-only
packages, sandboxed services.** Its init, package manager, service supervisor,
TUI panel and framebuffer compositor are all written from scratch, in Rust, Go
and Zig.

**No systemd. No glibc userland. No distro underneath.** Just a static busybox
rootfs, a Rust PID 1, and the tools in this repo.

---

## Status — read this first

| | |
|---|---|
| **Boots and runs?** | Yes — boots, mounts its disk, and unlocks on real hardware |
| **Safe for real use?** | **Not yet.** No independent security review |
| **A/B OS updates** | Written, never survived a real reboot — [details](docs/OS_UPDATES.md) |
| **Hardware coverage** | One machine. [Checklist here](docs/HARDWARE_VALIDATION.md) |

> **Treat this as a working prototype, not a product.** It does what it says,
> but nobody outside the project has audited it.

---

## Try it in a VM (5 minutes)

Needs an **Arch host** with root (the build uses `pacman`).

```bash
sudo ./build.sh                          # 1. build the ISO
./tools/bin/zrpkg-server -dir tools/repo &   # 2. serve packages

qemu-system-x86_64 -m 8G -enable-kvm \
    -drive file=vakt-data.img,format=raw,index=0,media=disk \
    -cdrom vakt-os.iso \
    -netdev user,id=n0 -device e1000,netdev=n0
```

That's it. You'll land on a PIN setup screen, then the panel.

> **Give it plenty of RAM.** The initramfs *is* the root filesystem, so it all
> has to fit in memory — and during boot the compressed copy and the unpacked
> one exist at once. Too little RAM and the kernel panics with
> `No working init found`, which reads like a broken image and isn't one. A
> `host`-kernel build carries the machine's driver and firmware trees and runs
> a few hundred MB; `VAKT_KERNEL=custom` is far smaller.

---

## Put it on real hardware

You need **two drives**: one to boot from, one for data. They can't be the
same drive.

**1. Build it** — use the default `host` kernel for real hardware:

```bash
sudo ./build.sh
```

**2. Find your drives.** Letters move between reboots, so check every time:

```bash
lsblk -o NAME,SIZE,MODEL,TRAN,LABEL
```

**3. Write the boot USB** — replace `sdX`:

```bash
sudo dd if=vakt-os.iso of=/dev/sdX bs=4M status=progress conv=fsync
```

**4. Format the data drive** — replace `sdY`, and **keep the label**:

```bash
sudo mkfs.ext4 -L VAKTDATA /dev/sdY
```

> **The `VAKTDATA` label is not optional.** GRUB and `vakt-init` both find the
> data disk by label, never by device name — device letters aren't stable.
> Get this wrong and the appliance silently boots into RAM-only mode.

**5. Boot it.** Hit your boot-menu key (`F12` / `F10` / `Esc`), pick the USB.

> **Turn Secure Boot off.** GRUB here isn't signed for it.

---

## If something goes wrong

| Symptom | Do this |
|---|---|
| Forgot the PIN | Boot the **root recovery shell** GRUB entry → `rm -f /persistent/etc/vakt-panel.auth` |
| Stuck in a launch/exit loop | It backs off after 3 tries and prints why — read the red text |
| `/persistent` not mounting | Check the data disk really has the `VAKTDATA` label |
| Panel won't start at all | Recovery shell entry always skips the panel |

Full runbook: **[docs/OPERATIONS.md](docs/OPERATIONS.md)**

---

## How it boots

```
GRUB → vmlinuz → initramfs → /init  (vakt-init, Rust, PID 1)
                                ├── mounts /proc /sys /dev, tmpfs over /run and /tmp
                                ├── loads hardware drivers
                                ├── mounts the disk labeled VAKTDATA → /persistent
                                ├── remounts / read-only
                                ├── supervises vakt-net + vakt-ids
                                └── drops to uid 1000, runs vakt-panel (TUI)
                                          └── vakt-compositor (/dev/fb0)
```

---

## What's in it

| Component | Lang | Does what |
|---|---|---|
| `vakt-init` | Rust | PID 1: mount, seal root, supervise, drop privilege, shut down |
| `vakt-net` | Rust | Wi-Fi/DHCP, Landlock sandboxed |
| `vakt-ids` | Go | Filesystem integrity monitor |
| `vakt-panel` | Go | PIN-protected TUI, runs unprivileged |
| `vakt-audit` | Go | CIS-style compliance checks |
| `vakt-compositor` | Rust | Draws to `/dev/fb0` via mmap, Landlock sandboxed |
| `zrpkg` | Rust | Packages: resolve, fetch, verify, install, remove |
| `zrpkg-server` | Go | HTTP repo server, rate-limited |
| `vakt-verify` | Zig | Independent, from-scratch signature re-check |
| `vakt-update` | Rust | A/B image updates — [unvalidated](docs/OS_UPDATES.md) |

---

## Security model

The five things this project exists to demonstrate:

1. **Read-only root.** `/` is remounted `ro` after boot.
2. **Panel runs unprivileged.** Drops to uid 1000, then verifies uid 0 is
   genuinely unreachable.
3. **Landlock sandboxing.** The compositor reaches only `/dev/fb0`; the
   network daemon only its own config.
4. **Signed packages only.** No warn-and-continue path. Ever.
5. **Independent second check.** `vakt-verify` re-implements signature
   verification in Zig, shares no code with `zrpkg`, and the build fails if
   the two disagree.

Plus: boot-time kernel hardening sysctls, and a PIN gate on the panel.

**There is no central repository, and no vendor key.** The first build on your
machine generates a signing key that never leaves it, and bakes *your* public
key into *your* image as the only key it will ever trust. Point it at a server
you run — `zrpkg repo <url>` — and the supply chain is yours end to end: you
sign, you host, your appliance verifies against your key. Nobody, including
this project, can publish something your appliance will install.

That is also why a prebuilt `vakt-os.iso` from the releases page cannot install
packages. It trusts whichever key the CI runner generated, and that key was
destroyed with the runner. The ISO is for looking at; **build your own to use
one.**

<details>
<summary><b>Known trade-offs (click)</b></summary>

- **Physical access wins.** The `vakt.rootshell` GRUB entry hands out a root
  shell. That's deliberate — it's the recovery path for a forgotten PIN. The
  PIN defends against a passer-by, not against someone with a screwdriver.
- **Wi-Fi password is plaintext at rest**, protected by root-only file
  permissions rather than encryption. `vakt-net` needs it at boot, before the
  PIN (the only real secret) has been entered — so there's no key available to
  encrypt it with that wouldn't break unattended boot. Reasoning in
  [ROADMAP.md](ROADMAP.md).
- **No independent review yet.** Everything found so far was found from
  inside the project.

</details>

---

## Everyday tasks

<details>
<summary><b>Packages</b></summary>

```bash
zrpkg update              # list what the repository offers
zrpkg list                # list what's installed here
zrpkg install <name>      # resolve, fetch, verify, install
zrpkg verify <name>       # check signature without installing
zrpkg remove <name>       # delete exactly what was installed
zrpkg repo <url>          # change the repository
```

Installs land in `/persistent/zrpkg`. Dependencies resolve automatically.
Removal is refused while something still depends on it (`--force` overrides).

Build and sign a repository:

```bash
./build-system/mkrepo.sh
```

First run generates a signing key at `build-system/keys/repo.key` (gitignored,
mode 0600). Every package is signed, then independently re-checked with
`vakt-verify` before it's published.

Host it yourself, anywhere:

```bash
zrpkg repo https://packages.example.com
./deploy/publish.sh user@vps.example.com
```

`publish.sh` signs locally and copies only the signed output, so **the signing
key never touches the server**. A server that is fully compromised still cannot
produce a package your appliance will accept — it can withhold updates or serve
stale ones, which is why the appliance verifies rather than trusts.

For one appliance on your own network you do not need a server at all: run
`zrpkg-server -dir tools/repo` on the build machine and point the appliance at
its address.

See [`deploy/README.md`](deploy/README.md) for the systemd unit, TLS, and rate
limiting.

</details>

<details>
<summary><b>Wi-Fi</b></summary>

Set it from the panel's **Wi-Fi Setup** page, or write
`/persistent/etc/vakt-net.conf` by hand:

```ini
ssid=MyNetwork
psk=hunter2
interface=wlan0
```

With no config at all, `vakt-net` just does DHCP on `eth0`.

> Wi-Fi needs `VAKT_KERNEL=host`. The `custom` kernel carries no chipset
> firmware, on purpose.

</details>

<details>
<summary><b>Backups</b></summary>

Both ship inside the image, so they work with no network and no extra tooling:

```bash
vakt-backup  /persistent /mnt/usb/backup-$(date +%F).tar.gz
vakt-restore /mnt/usb/backup-2026-08-07.tar.gz /persistent
```

Restore verifies a SHA-256 checksum before touching anything, and refuses a
destination that isn't empty.

</details>

<details>
<summary><b>OS updates (unvalidated)</b></summary>

> **Never survived a real reboot.** Read [docs/OS_UPDATES.md](docs/OS_UPDATES.md)
> before using this on anything you can't recover by hand.

A/B updates for the kernel and image itself, not just packages. The boot
medium (slot A) is never written to; an update lands as slot B on
`/persistent`, and `vakt-init` rolls back on its own if slot B never reaches a
working boot.

```bash
sudo ./build-system/mkupdate.sh 1.1.0   # build + sign a new slot B
vakt-update check                       # on the appliance
vakt-update apply --reboot
```

</details>

---

## Build options

| | `VAKT_KERNEL=host` *(default)* | `VAKT_KERNEL=custom` |
|---|---|---|
| Kernel | The build machine's own | Built from `build-system/kernel.config` |
| Hardware | Whatever the host supports | QEMU, common wired NICs, NVMe/AHCI, USB |
| Wi-Fi | Yes | **No** — no firmware blobs |
| Use it for | Real hardware | A VM, or one fixed known machine |

```bash
sudo VAKT_KERNEL=custom ./build.sh
sudo VAKT_REPO_URL=https://packages.example.com ./build.sh   # bake in a repo URL
```

> `vakt-data.img` is never recreated if it already exists — it holds your
> Wi-Fi credentials, packages, and IDS baseline. Delete it to start clean.

---

## Tests

```bash
cargo test --manifest-path vakt-init/Cargo.toml
cargo test --manifest-path pkg-manager/Cargo.toml
cargo test --manifest-path vakt-net/Cargo.toml
cargo test --manifest-path vakt-update/Cargo.toml
cd tools && go test ./cmd/...
cd vakt-verify && zig build test
```

CI runs all of that, then a full package pipeline (pack → sign → install →
tamper detection), then builds the ISO, then builds and applies a signed A/B
update bundle end to end. Tagging `v*` publishes the ISO as a release asset.

<details>
<summary><b>Repo layout</b></summary>

```
build.sh                    Full ISO build
build-system/mkkernel.sh    Builds the monolithic kernel
build-system/mkrepo.sh      Builds and signs the package repository
build-system/mkupdate.sh    Builds and signs an A/B update bundle (slot B)
build-system/checkimage.sh  Checks a packed initramfs without booting it
pkg-manager/                zrpkg (Rust)
pkg-manager/fuzz/           cargo-fuzz targets for untrusted-input parsing
vakt-init/                  PID 1, supervisor, readiness, shutdown (Rust)
vakt-net/                   Networking daemon (Rust)
vakt-compositor/            Framebuffer compositor (Rust)
vakt-verify/                Independent signature verifier (Zig)
vakt-update/                A/B OS image updater (Rust)
tools/cmd/                  Go tools: panel, audit, ids, repo server
tools/vakt-backup           Backs up /persistent (ships in the image)
tools/vakt-restore          Restores a vakt-backup archive (ships in the image)
deploy/                     Running the repository on a rented server
docs/OPERATIONS.md          Runbook: lockouts, crash loops, IDS alerts, backups
docs/SECURITY_AUDIT.md      Findings, unsafe-block review, fuzzing notes
docs/HARDWARE_VALIDATION.md Checklist for testing on real hardware
docs/NEXT.md                What's being worked on next, and what isn't
docs/OS_UPDATES.md          A/B update design and what's actually verified
.github/workflows/build.yml CI: tests, package pipeline, ISO artifact
```

</details>

<details>
<summary><b>Third-party components</b></summary>

Almost everything here is written from scratch. What isn't:

| Component | Source | License |
|---|---|---|
| busybox 1.35.0 (static) | [busybox.net](https://www.busybox.net/downloads/binaries/1.35.0-x86_64-linux-musl/busybox) | GPL-2.0 |
| Linux kernel | [kernel.org](https://kernel.org) | GPL-2.0 |
| GRUB | Arch package | GPL-3.0 |
| wpa_supplicant | Arch package ([w1.fi](https://w1.fi/wpa_supplicant/)) | BSD-3-Clause |
| CA certificates | Arch `ca-certificates` | MPL-2.0 |

Rust: `nix`, `landlock`, `clap`, `anyhow`, `tokio`, `reqwest`, `serde`,
`ed25519-dalek`, `sha2`, `hex`, `tar`, `flate2`, `libc`, `memmap2`.
Go: [`rivo/tview`](https://github.com/rivo/tview),
[`gdamore/tcell`](https://github.com/gdamore/tcell).
Zig: none — `vakt-verify` uses only the standard library.

The readiness protocol borrows systemd's
[`sd_notify`](https://www.freedesktop.org/software/systemd/man/sd_notify.html)
wire format (no code). Kernel hardening options follow the
[Kernel Self Protection Project](https://kernsec.org/wiki/index.php/Kernel_Self_Protection_Project/Recommended_Settings).

CI: `actions/checkout`, `actions/cache`, `actions/upload-artifact`,
[`softprops/action-gh-release`](https://github.com/softprops/action-gh-release).

</details>

---

## License

[GNU Affero General Public License v3.0](LICENSE), copyright 2026 jaysyrk.

Use it, study it, modify it, run it, redistribute it. The condition is that
anything you build on it stays under the same license and ships with its
source — including when people reach a modified version over a network, which
is the part the Affero clause adds over the plain GPL.

**Commercial licenses are available.** If the AGPL doesn't suit what you want
to build, the copyright holder can license this code to you under different
terms — [open an issue](https://github.com/jaysyrk/vakt-os/issues) to ask.
Being the sole author is what makes that possible.

Third-party components keep their own licenses — see **Third-party components**
above. The built image carries GPL-2.0 software (busybox, the Linux kernel)
alongside this project's own binaries: separate programs sharing a disk, not a
combined work.
