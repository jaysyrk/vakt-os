# Vakt OS

[Readme](README.md) | [Roadmap](ROADMAP.md) | [Contributing](CONTRIBUTING.md)

**Claude was used solely for .md formats, .sh scripts, and git push automation**

A Linux security appliance built from scratch — custom init, package manager,
TUI panel, service supervisor, and framebuffer compositor, in Rust, Go, and
Zig. No systemd, no glibc userland, no distro underneath: a static busybox
rootfs, a Rust PID 1, and the tools in this repository.

```
GRUB → vmlinuz → initramfs → /init (vakt-init, Rust, PID 1)
                                ├── mounts /proc /sys /dev, tmpfs over /run and /tmp
                                ├── mounts /dev/sda → /persistent
                                ├── remounts / read-only
                                ├── supervises vakt-net (Wi-Fi/DHCP) and vakt-ids (integrity)
                                └── drops to uid 1000, runs vakt-panel (TUI)
                                          └── vakt-compositor (/dev/fb0)
```

## Components

| Component | Language | Role |
|---|---|---|
| `vakt-init` | Rust | PID 1: mount, seal root, supervise, drop privilege, shut down. |
| `vakt-net` | Rust | Wi-Fi/DHCP, Landlock sandboxed. |
| `vakt-ids` | Go | Filesystem integrity monitor. |
| `vakt-panel` | Go | PIN-protected TUI, runs unprivileged. |
| `vakt-audit` | Go | CIS-style compliance checks. |
| `vakt-compositor` | Rust | Draws to `/dev/fb0` via mmap, Landlock sandboxed. |
| `zrpkg` | Rust | Package manager: resolve, fetch, verify, install, remove. |
| `zrpkg-server` | Go | HTTP repository server, rate-limited. |
| `vakt-verify` | Zig | Independent, from-scratch signature re-check. |

## Security model

- **Read-only root.** `/` is remounted `ro` after boot. `/run`/`/tmp` are
  size-capped tmpfs; `/persistent` is the data disk.
- **Panel runs unprivileged.** `vakt-init` drops to uid 1000 before launching
  it, and verifies uid 0 is unreachable afterward. `vakt.rootshell` on the
  kernel command line gives a recovery shell if the panel won't start.
- **PIN-protected panel.** A PIN is required before the menu appears — set on
  first boot, changed from the Panel Lock page.
- **Landlock sandboxing.** `vakt-compositor` can reach only `/dev/fb0`.
  `vakt-net` can reach only its own config file under `/persistent`.
- **Signed packages only.** `zrpkg` refuses anything that doesn't verify
  against `/etc/vakt/trusted.key` — no warn-and-continue path.
- **Independent signature check.** `vakt-verify` re-implements the same check
  in Zig, no code shared with `zrpkg`, and the build fails if they disagree.
- **Boot-time kernel hardening.** `vakt-init` applies hardening sysctls
  (`ptrace_scope`, `kptr_restrict`, `rp_filter`, etc.) at startup.

## Building

Requires an Arch host (uses `pacman`) and root.

```bash
sudo ./build.sh
```

Two kernel modes:

| | `VAKT_KERNEL=host` (default) | `VAKT_KERNEL=custom` |
|---|---|---|
| Kernel/modules | Build machine's own | Built from `build-system/kernel.config`, monolithic |
| Hardware | Whatever the host supports | QEMU, common wired NICs, NVMe/AHCI, USB — no Wi-Fi |

Use `host` on (or near) the machine you're deploying to. Use `custom` for a
VM or one fixed, known machine:

```bash
sudo VAKT_KERNEL=custom ./build.sh
sudo VAKT_REPO_URL=https://packages.example.com ./build.sh   # bake in a repo URL
```

`vakt-data.img` is not recreated if it already exists — it holds Wi-Fi
credentials, packages, and the IDS baseline. Delete it to start clean.

## Running

**In a VM:**

```bash
./tools/bin/zrpkg-server -dir tools/repo
qemu-system-x86_64 -m 2G -enable-kvm \
    -drive file=vakt-data.img,format=raw,index=0,media=disk \
    -cdrom vakt-os.iso \
    -netdev user,id=n0 -device e1000,netdev=n0
```

**On real hardware:**

```bash
sudo dd if=vakt-os.iso of=/dev/sdX bs=4M status=progress conv=fsync   # boot USB
sudo mkfs.ext4 /dev/sdY                                               # persistent disk
```

Disable Secure Boot (GRUB here isn't signed for it). Point `zrpkg` at a real
repository — there's no QEMU gateway on real hardware.

## Networking

`vakt-net` runs in the background: DHCP on `eth0` with no config, or Wi-Fi if
`/persistent/etc/vakt-net.conf` exists (set from the panel's Wi-Fi Setup
page):

```ini
ssid=MyNetwork
psk=hunter2
interface=wlan0
```

Wi-Fi needs `VAKT_KERNEL=host` — `custom` has no chipset firmware.

## Packages

```
zrpkg update              # list what's available
zrpkg install <name>      # resolve, fetch, verify, install
zrpkg verify <name>       # check signature without installing
zrpkg remove <name>       # delete exactly what was installed
zrpkg repo <url>          # change the repository
```

Installs go to `/persistent/zrpkg`. Dependencies are resolved and installed
in order. Removal is refused if another package still depends on it
(`--force` to override).

Build and sign the repository:

```bash
./build-system/mkrepo.sh
```

Generates a signing key on first run (`build-system/keys/repo.key`,
gitignored), signs every package, and re-checks each one with `vakt-verify`
before publishing.

**Remote repositories** — point at a rented server instead of the QEMU
default:

```bash
zrpkg repo https://packages.example.com
./deploy/publish.sh user@vps.example.com
```

See [`deploy/README.md`](deploy/README.md) for the systemd unit, TLS, and
rate limiting.

## Layout

```
build.sh                    Full ISO build
build-system/mkkernel.sh    Builds the monolithic kernel
build-system/mkrepo.sh      Builds and signs the package repository
pkg-manager/                zrpkg (Rust)
vakt-init/                  PID 1, supervisor, readiness, shutdown (Rust)
vakt-net/                   Networking daemon (Rust)
vakt-compositor/            Framebuffer compositor (Rust)
vakt-verify/                Independent signature verifier (Zig)
tools/cmd/                  Go tools: panel, audit, ids, repo server
deploy/                     Running the repository on a rented server
.github/workflows/build.yml CI: tests, package pipeline, ISO artifact
```

## Tests

```bash
cargo test --manifest-path vakt-init/Cargo.toml
cargo test --manifest-path pkg-manager/Cargo.toml
cargo test --manifest-path vakt-net/Cargo.toml
cd tools && go test ./cmd/...
cd vakt-verify && zig build test
```

CI runs all of these, then a full package pipeline (pack, sign, install,
tamper detection), then builds the ISO. Tagging `v*` publishes it as a
release asset.

## Third-party components

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

## License

[Apache License 2.0](LICENSE). Use, modify, and redistribute freely —
including commercially — as long as you keep the copyright/license notices
(see [NOTICE](NOTICE)) and state what you changed. Third-party components
keep their own licenses.
