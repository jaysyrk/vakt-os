# Real Hardware Validation

CI proves the ISO builds and boots under QEMU. It doesn't prove Secure Boot
behaves, that a given Wi-Fi chipset associates, or that a specific storage
controller is found at boot — those need a real machine. This is the
checklist for that pass: what to check, not just whether it boots.

Use `VAKT_KERNEL=host` for real hardware — the `custom` monolithic kernel
carries no Wi-Fi chipset firmware and is meant for QEMU or one fixed, known
machine (see the README's Building section).

Work through this on hardware you're willing to have wiped: the build writes
a boot USB and expects a second disk it will `mkfs.ext4` for `/persistent`.

## 1. Build and media

```bash
sudo ./build.sh                       # VAKT_KERNEL=host is the default
sudo dd if=vakt-os.iso of=/dev/sdX bs=4M status=progress conv=fsync
sudo mkfs.ext4 /dev/sdY               # the persistent data disk, separate device
```

- [ ] Build completes without falling back to any `custom`-kernel-only
      behavior you didn't ask for.
- [ ] `dd` finishes and the USB boots on the target machine at all (BIOS/UEFI
      sees it as a boot device).

## 2. Secure Boot

The README says GRUB here isn't signed for it — confirm what that means in
practice on real firmware, not just that it's disabled.

- [ ] With Secure Boot **on**, the USB either refuses to boot cleanly or the
      firmware clearly reports "not trusted" — no silent partial boot.
- [ ] With Secure Boot **off**, boot proceeds normally.
- [ ] Note the exact firmware error text if Secure Boot rejects it — worth
      recording in case it's ever worth signing GRUB properly.

## 3. Boot sequence

- [ ] GRUB menu shows both entries: **Vakt OS** and **Vakt OS (root recovery
      shell)**.
- [ ] Default entry (**Vakt OS**) boots to the panel's PIN screen (first-boot
      setup screen on a fresh `/persistent`).
- [ ] Recovery entry boots to a root shell instead (confirms `vakt.rootshell`
      on the kernel command line is actually read by this hardware's GRUB,
      not just in QEMU).
- [ ] Boot log shows `/` remounted read-only, `/persistent` mounted from the
      real second disk (not falling back to RAM-only mode because the disk
      wasn't found — see Storage below if it does).

## 4. Storage controller

The kernel needs to actually find `/persistent` on this machine's storage
hardware — NVMe, AHCI/SATA, and USB storage are the three the `host` kernel
config targets.

- [ ] `cat /run/services.status` (or the panel) shows `/persistent` mounted,
      not "no watched directory exists" from `vakt-ids` — that message means
      the disk was never found.
- [ ] If it's NVMe: confirms the `custom` kernel's NVMe config actually
      matches this drive, if you're also testing `VAKT_KERNEL=custom` on this
      same machine.
- [ ] Note the controller/drive model either way — useful if a future
      machine's storage isn't found and this list needs to grow.

## 5. Wired networking

- [ ] `cat /run/vakt-net.status` shows `connected` with DHCP, no
      `vakt-net.conf` present, on plain Ethernet.
- [ ] Confirms the NIC's driver is present in whichever kernel mode you
      built (`host` uses the machine's own modules; `custom` needs the NIC's
      `CONFIG_` symbol already in `build-system/kernel.config`).

## 6. Wi-Fi (requires `VAKT_KERNEL=host`)

- [ ] From the panel's Wi-Fi Setup page, write real SSID/PSK and confirm
      `vakt-net` associates: `cat /run/vakt-net.status` reaches `connected`.
- [ ] `cat /run/vakt-net.log` for `wpa_supplicant` output — confirm the
      chipset's firmware actually loaded (a chipset with no firmware in the
      host system won't associate, and the log will say so).
- [ ] Reboot and confirm it reconnects automatically from the saved
      `/persistent/etc/vakt-net.conf` with no prompts.

## 7. Panel and input

- [ ] Physical keyboard input reaches the panel correctly (QEMU's input path
      isn't always representative of a real keyboard/USB controller).
- [ ] Framebuffer output looks correct at this machine's actual display
      resolution — `vakt-compositor` reads geometry via `ioctl` at runtime,
      so this is worth checking on hardware with a resolution QEMU wouldn't
      have exercised.
- [ ] PIN entry, first-boot setup, and Panel Lock page all work as documented
      in [OPERATIONS.md](OPERATIONS.md).

## 8. Package install and IDS

- [ ] `zrpkg install <name>` against a real repository (not the QEMU-only
      gateway address) succeeds end to end: fetch, verify, install.
- [ ] `vakt-ids` picks up the new files on its next scan and does **not**
      alert on them (the baseline update path, not a false positive).
- [ ] Deliberately modify a watched file by hand and confirm an alert fires.

## 9. Shutdown

- [ ] Panel-initiated reboot/poweroff/halt all work (this is the path that
      exercises `/run/init.sock`'s `SHUTDOWN=` message, unprivileged panel to
      privileged PID 1 — confirm it isn't silently swallowed on this
      hardware).
- [ ] After poweroff, confirm the machine actually powers off rather than
      hanging (a kernel missing the right ACPI/power-off support for this
      board would hang here, not fail loudly).

## OS image A/B updates

Separate from the sections above: the A/B update mechanism
([OS_UPDATES.md](OS_UPDATES.md)) is implemented but has never been through an
actual reboot as of this writing - nothing before this point in the document
depends on it, but this section does, and it deserves care neither of us has
been able to give it yet. Work through this only after sections 1-9 above
pass, on hardware you are prepared to recover by hand (know the
`vakt.rootshell` GRUB entry works on this machine *before* starting).

1. **Build a slot-B bundle**: `sudo ./build-system/mkupdate.sh test-1.0`,
   then serve it (`zrpkg-server -dir tools/repo`) and confirm
   `vakt-update check` from the running appliance sees it.
2. **Apply it**: `vakt-update apply` (without `--reboot` first). Confirm on
   the appliance:
   - [ ] `/persistent/vakt-update/B/vmlinuz` and `initramfs.img` exist.
   - [ ] `/persistent/etc/vakt-update-state.json` contains `tries_left: 3`.
   - [ ] `/persistent/etc/vakt/bootenv` exists (`grub-editenv` isn't shipped
         in the image, so eyeball it with `strings` or `cat -v`; it should
         contain `vakt_active=B`).
3. **Reboot and confirm slot B actually boots** - the first thing that has
   never been tested for real:
   - [ ] GRUB's menu (or its default, if the timeout is too short to see)
         resolves to entry 2, "Vakt OS (update slot B)".
   - [ ] The appliance boots into slot B's kernel/initramfs, not slot A's -
         worth confirming a version marker differs if you built slot B from
         changed sources.
   - [ ] Boot reaches the panel normally.
   - [ ] After boot, `/persistent/etc/vakt-update-state.json` is **gone**
         (confirmed) rather than still present.
4. **Reboot again** (a normal, successful reboot of the now-confirmed slot
   B): confirm it boots slot B again, and stays confirmed - `vakt_active`
   in the bootenv should still read `B`, and no state file should reappear.
5. **Test the rollback path deliberately** - this is the one that matters
   most and is hardest to exercise by accident:
   - Build and apply a slot-B bundle that **cannot** reach readiness (e.g.
     temporarily point it at a repository URL that doesn't exist, or break
     one of the `DEFAULT_SERVICES` binaries in a throwaway build) and
     reboot.
   - [ ] The appliance attempts slot B, fails to confirm, and - within 3
         boot attempts - vakt-init writes `vakt_active=A` and reboots on
         its own, with no operator action.
   - [ ] The next boot is slot A, and the appliance is otherwise unharmed
         (panel comes up, `/persistent` data intact).
   - [ ] `/persistent/etc/vakt-update-state.json` is gone after the
         rollback.
6. **Record exact behavior**, especially anything that *doesn't* match
   [OS_UPDATES.md](OS_UPDATES.md)'s description - a GRUB version whose
   `search`/`load_env` syntax works differently, a boot-attempt count that
   turns out too aggressive or too lax, a race between vakt-init's rollback
   write and GRUB reading it. This section only reflects design intent until
   someone has actually done this once.

## Recording results

There's no fixed template — a short note per machine (model, what passed,
what didn't, and the exact error text for anything that failed) is enough to
be useful to whoever reads this next. If a class of hardware turns out to be
unsupported (a storage controller, a Wi-Fi chipset), that's a
`build-system/kernel.config` change, not a documentation-only fix.
