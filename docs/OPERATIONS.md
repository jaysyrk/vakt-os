# Operations Runbook

For running a deployed appliance. If you want to know how it works inside,
that's the [README](../README.md).

---

## Something's wrong — start here

| What you're seeing | Go to |
|---|---|
| Locked out — forgot the PIN | [Get back in](#get-back-in) |
| Panel opens and closes over and over | [The console loop](#the-console-loop) |
| PIN is correct but won't unlock | [Get back in](#get-back-in) |
| A service says `failed` | [A service won't stay up](#a-service-wont-stay-up) |
| No network | [Network won't come up](#network-wont-come-up) |
| IDS alert fired | [An IDS alert fired](#an-ids-alert-fired) |
| Data disk missing / changes don't survive reboot | [The disk isn't mounting](#the-disk-isnt-mounting) |
| You think it's compromised | [Start over safely](#start-over-safely) |

---

## The emergency card

**Get a root shell, always:** reboot → GRUB menu → **Vakt OS (root recovery
shell)**. It skips the panel entirely and hands you a root prompt. It works
even when the panel is broken, locked, or crash-looping.

> Skipping the panel needs a build from the current `main`. Older images
> booted the panel first even with `vakt.rootshell` set, so the PIN screen
> came up anyway. If that's what you're seeing, rebuild.

**Is it healthy?**

```bash
cat /run/services.status      # each service: state, pid, restarts, readiness
cat /run/vakt-net.status      # link state, address
tail -50 /run/vakt-ids.alerts # recent integrity findings
zrpkg repo                    # which repository this appliance fetches from
```

Healthy looks like: every service `running` + `ready`, network `connected`,
no unexplained IDS findings.

---

## Get back in

You forgot the PIN, or the correct PIN won't work.

1. Reboot → GRUB menu → **Vakt OS (root recovery shell)**
2. Delete the PIN:
   ```bash
   rm -f /persistent/etc/vakt-panel.auth
   reboot
   ```
3. Boot normally. You'll get the first-boot setup screen.

If no data disk ever mounted, the PIN lives at `/etc/vakt/panel.auth` in RAM
instead — and is already gone at the next reboot either way.

**No console access?** Pull the data disk, plug it into another Linux
machine, and do the same thing there:

```bash
sudo mkdir -p /mnt/vakt
sudo mount -L VAKTDATA /mnt/vakt
sudo rm -f /mnt/vakt/etc/vakt-panel.auth
sudo umount /mnt/vakt
```

> **Press Enter and nothing happens?** On builds before this was fixed, Enter
> in the PIN field only moved focus to the **Unlock** button — no attempt, no
> error, no counter. Press Enter a second time, or Tab to **Unlock** and press
> it. A correct PIN looked broken. Current builds submit on the first Enter.

<details>
<summary>Why a correct PIN can stop working</summary>

If the auth file gets damaged — a power cut mid-write, a bad disk — nothing
verifies against it, including the right PIN.

Current builds detect this: an unreadable auth file is treated as *no PIN*
and takes you to the setup screen with a **red** message saying the old one
is gone. If you get a normal-looking lock screen instead, the file is intact
and the PIN really is wrong.

A healthy file is one line, 32 hex characters, a colon, then 64 more:

```bash
cat /persistent/etc/vakt-panel.auth
```

**Or the file is fine and the panel just cannot read it.** It must be owned by
the panel's user, not by root — a PIN set while the panel was running as root
stays root-owned and 0600, and the panel then gets `Permission denied` on
every check. Current builds adopt it at boot and say so in red if they can't;
older ones showed an ordinary setup screen and refused the correct PIN with no
explanation.

```bash
stat -c 'uid=%u gid=%g mode=%a' /persistent/etc/vakt-panel.auth   # want uid=1000
chown vakt:vakt /persistent/etc/vakt-panel.auth                   # from a root shell
```

Off the appliance, with the disk mounted elsewhere, `chown 1000:1000` on the
same file does the same thing — the PIN itself is unaffected.

</details>

> Anyone with physical access can do all of the above. That's deliberate —
> see the README's Security model. The PIN stops a passer-by, not someone
> with a screwdriver.

---

## The console loop

The panel starts, exits immediately, drops to a shell, and repeats.

**It will stop on its own.** After 3 fast rounds it backs off, waits, and
prints the reason in red. Read that text — it names the exit status and, if
the console couldn't be handed to the panel's user, says so.

Most likely causes:

| Message | Meaning |
|---|---|
| `Could not run ...: Permission denied` | The panel's user can't execute or reach the console — a build problem, not a runtime one. Rebuild from a current checkout. |
| `... exited with status N` | The panel started and quit. Check the recovery shell for its output. |
| `Could not give /dev/console to vakt` | `vakt-init` couldn't hand over the console device. |

Get in with the recovery shell entry and investigate from there.

---

## A service won't stay up

`vakt-init` stops restarting a service after **5 crashes in 60 seconds** and
marks it `failed`.

1. Read its log:
   ```bash
   cat /run/<name>.log          # and /run/<name>.log.1 for the previous one
   ```
2. Match the cause:

| Service | Usual cause |
|---|---|
| `vakt-net` | Missing or malformed `/persistent/etc/vakt-net.conf`. Rewrite it from the panel's Wi-Fi Setup page, or check `ssid=` / `psk=` / `interface=` by hand. |
| `vakt-ids` | `/persistent` never mounted — see [The disk isn't mounting](#the-disk-isnt-mounting). Not a crash; it just has nothing to watch. |
| A `zrpkg` package | Reinstall it: `zrpkg remove <name> && zrpkg install <name>` |

3. There's no live "reset the restart count" command, by design. Reboot.

---

## The disk isn't mounting

Symptom: changes don't survive reboot, `vakt-ids` has nothing to watch, or
the boot log says:

```
No disk labeled 'VAKTDATA' found. Running in RAM only mode.
```

The data disk is found **by filesystem label, never by device name** —
device letters aren't stable between boots. Check from the recovery shell or
another machine:

```bash
lsblk -o NAME,SIZE,LABEL
```

If the label is missing, that's the problem. Re-label an existing ext4 disk
without wiping it:

```bash
sudo e2label /dev/sdX VAKTDATA
```

---

## Network won't come up

1. Check the state:
   ```bash
   cat /run/vakt-net.status
   ```
   - `unconfigured` — no Wi-Fi set up and no wired link. Expected on a fresh
     appliance.
   - `failed` — it tried and couldn't.
2. **Wi-Fi needs `VAKT_KERNEL=host`.** The `custom` kernel ships no chipset
   firmware, on purpose.
3. More detail:
   ```bash
   cat /run/vakt-net.log        # wpa_supplicant / udhcpc output
   ```

---

## An IDS alert fired

`vakt-ids` reports `ADDED`, `MODIFIED`, `DELETED`, or `PERMISSIONS` against
its SHA-256 baseline of `/persistent`.

```bash
tail -50 /run/vakt-ids.alerts
```

Or the panel's Intrusion Detection page.

**Was it you?** (installed a package, edited a config) — nothing to do. The
baseline catches up on the next scan; a repeat scan with no finding confirms
it.

**Was it not you?** Treat the appliance as compromised. This tool tells you
something changed; it can't tell you it's safe. Go to
[Start over safely](#start-over-safely).

---

## Start over safely

There's deliberately no in-place "repair" for an appliance you no longer
trust. Don't reason your way to certainty on a machine that might be lying
to you.

1. **Reimage** from a known-good `vakt-os.iso`:
   ```bash
   sudo dd if=vakt-os.iso of=/dev/sdX bs=4M status=progress conv=fsync
   ```
2. **Restore `/persistent` from a backup made *before* the incident** — not
   from the current disk, which would carry the problem forward.
3. **Rotate the signing key** if it might have been exposed. It lives only on
   the build machine, so this is usually unnecessary — unless that machine
   was in scope too. See
   [`deploy/README.md`](../deploy/README.md#rotating-the-signing-key).
4. **Set a new PIN** rather than restoring the old `vakt-panel.auth`.

---

## Backups

Both tools ship inside the image — no network, no extra install.

```bash
vakt-backup  /persistent /mnt/usb/vakt-backup-$(date +%F).tar.gz
vakt-restore /mnt/usb/vakt-backup-2026-08-07.tar.gz /persistent
```

> **Store it somewhere else.** A backup on the disk it's backing up survives
> none of the failures worth planning for.

Nothing runs backups automatically — no timer ships by default. `vakt-restore`
checks the archive's SHA-256 before touching anything, refuses a corrupted
one outright, and refuses to write into a destination that isn't empty.

---

<details>
<summary><b>Updating the OS image itself</b></summary>

> **Never survived a real reboot yet.** Read [OS_UPDATES.md](OS_UPDATES.md)
> first, and confirm the recovery GRUB entry works on this specific machine
> before you try it.

`zrpkg` updates packages. It does **not** update the kernel, `vakt-init`, or
anything else baked into the image — that's `vakt-update`.

Slot A (the boot medium) is never written to. An update lands as slot B on
`/persistent`, and `vakt-init` rolls back to slot A on its own if slot B
doesn't reach a working boot within 3 attempts. No operator action needed for
a bad update to correct itself.

```bash
vakt-update check     # what's staged, what's available
vakt-update apply     # fetch, verify, stage slot B
reboot
```

After rebooting, **confirm it actually came up** (panel reachable,
`/run/services.status` healthy) before trusting the update. If it silently
fails to reach readiness, the rollback happens on the *next* reboot, not this
one.

There's no "roll back now" command — to force it, reimage from slot A.

</details>

<details>
<summary><b>Sending IDS alerts to a webhook (fleet setups)</b></summary>

For more than one appliance. Off by default.

`vakt-init` supervises `vakt-ids` with a fixed argument list, so the URL goes
in a one-line config file rather than a flag:

```bash
echo "https://your-collector.example.com/vakt-ids" > /persistent/etc/vakt-ids-webhook.conf
```

Picked up on the next `vakt-ids` start. Each alert POSTs:

```json
{"host": "appliance-3", "time": "2026-08-07T12:00:00Z", "kind": "MODIFIED", "detail": "/persistent/etc/passwd"}
```

Best-effort, 5-second timeout, no retry. A slow or unreachable collector logs
a warning and never blocks scanning or crashes the daemon — the alert file
stays the durable record.

</details>

<details>
<summary><b>Rotating the package-signing key</b></summary>

A repository-side operation, not something done on a running appliance:
regenerate the key, rebuild and reship images carrying the new trust anchor.

Full procedure in
[`deploy/README.md`](../deploy/README.md#rotating-the-signing-key).

</details>
