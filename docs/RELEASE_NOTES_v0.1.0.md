# Vakt OS v0.1.0

The first tagged build. A small immutable Linux security appliance: read-only
root, signed-only packages, sandboxed services. The init, package manager,
service supervisor, network daemon and package verifier are written from
scratch — Rust, Go and Zig — with no systemd, no distribution userland beyond
busybox, and no unsigned code path.

This is a first release of a young project. The section on what has **not**
been validated is the important one, and it is longer than the feature list on
purpose.

## What it does

- **Read-only root.** The initramfs is the root filesystem and is sealed after
  boot. Writes fail rather than being silently redirected.
- **Signed-only packages.** `zrpkg` verifies an Ed25519 signature against
  `/etc/vakt/trusted.key` before anything is unpacked. A tampered archive with
  a valid manifest is refused. The repository URL is not a trust decision — a
  hostile mirror gets you failed signature checks, not compromised packages.
- **Sandboxed services.** `vakt-net` and `vakt-compositor` confine themselves
  with Landlock before spawning any helper, and the ruleset is inherited by
  everything they run. It cannot be widened from inside.
- **Privilege separation.** The panel and everything it launches run as an
  unprivileged user. PID 1 keeps root; nothing else asks for it.
- **A supervisor with readiness.** Daemons report readiness over a
  `sd_notify`-shaped socket, so the panel appears when the system is usable
  rather than after a guessed delay. Output is captured to size-capped logs on
  a tmpfs.
- **Console PIN lock**, integrity monitoring (`vakt-ids`), a system audit
  (`vakt-audit`), and backup/restore for the persistent disk.

## What is verified, and how

| Area | Verified on |
|---|---|
| Boot, storage, panel, PIN lock, read-only root | real hardware |
| IDS detection and alerting | host and a booted appliance |
| Console PIN: set, reboot, unlock, reject a wrong PIN | QEMU, full round trip |
| Landlock actually enforced (not just requested) | QEMU, asserted by the boot test |
| Wi-Fi: WPA2 association and DHCP | a **virtual** radio (`mac80211_hwsim`) |
| Wired DHCP | QEMU |
| Panel-initiated shutdown | QEMU |
| `zrpkg` install / verify / remove | host |

CI builds the image and then **boots it**, checking that it reaches a shell,
seals the root, starts both daemons, can exec as the unprivileged user, and
that no path the sandbox granted is still denied.

## What is not verified

Read this part.

- **It has been booted on one machine.** Mine. If it does not boot on yours,
  that is expected rather than surprising, and a report is genuinely useful.
- **No real Wi-Fi chipset has been tested.** Association is verified against a
  virtual radio, which exercises everything above the driver and nothing
  below it. A card whose firmware the image does not carry will enumerate and
  never work. The build now warns when a loaded driver's firmware is missing.
- **`zrpkg` has never installed a package from a booted appliance** — only
  from a build host.
- **Secure Boot has never been attempted.**
- **A/B image updates have never survived a reboot.** The code is present and
  documented; treat it as unfinished.
- **The framebuffer compositor has never run on a real display.**

## Known issues

- `vakt-init` prints to the same console the panel and shell use, so a service
  reporting ready can splice a line into what you are typing. Cosmetic, but it
  makes the console confusing exactly when you are debugging.
- The panel cannot configure a wired-only machine: its network page requires an
  SSID. A cable with no configuration is picked up automatically via DHCP.
- `dmesg` is unreadable for the panel's user by design (`dmesg_restrict`). Use
  the root recovery shell.

## Installing

Build on Arch — the build script installs its dependencies with `pacman`:

```sh
git clone https://github.com/jaysyrk/vakt-os
cd vakt-os
sudo ./build.sh
sudo dd if=vakt-os.iso of=/dev/sdX bs=4M status=progress conv=fsync
```

`VAKT_KERNEL=host` (the default) reuses the running kernel and its modules, so
your hardware works. `VAKT_KERNEL=custom` builds a monolithic kernel from
`build-system/kernel.config` — much smaller, but it carries no Wi-Fi chipset
drivers at all.

**This wipes the target device.** Check `lsblk` twice.

## If it does not work

`/run/vakt-net.log` and `/run/vakt-net.status` say what the network is doing
and why it failed. The GRUB menu has a root recovery shell entry for
everything else. Hardware reports are welcome — there is an issue template,
and the one question that matters most is which Wi-Fi chipset you have.

## Licence

AGPL-3.0.
