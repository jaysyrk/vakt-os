# Operations Runbook

For anyone running a deployed appliance day to day, not building or
contributing to it. If you're looking for how the system works internally,
see the [README](../README.md) and the wiki instead.

## Health check

From the panel, or a shell if you have one:

```bash
cat /run/services.status      # supervisor's view: state, pid, restarts, readiness
cat /run/vakt-net.status      # link state, address
tail -50 /run/vakt-ids.alerts # recent integrity findings
zrpkg repo                    # which repository this appliance fetches from
```

A healthy appliance shows every supervised service as `running` with
`ready`, a `connected` network state, and no unexplained `vakt-ids` findings.

## Locked out of the panel (forgotten PIN)

1. Reboot and pick **Vakt OS (root recovery shell)** from the GRUB menu
   instead of the plain **Vakt OS** entry. This boots with `vakt.rootshell`
   on the kernel command line, which hands the console a root shell instead
   of the panel.
2. Delete the PIN file:
   ```bash
   rm -f /persistent/etc/vakt-panel.auth
   ```
   (If the appliance never had a persistent disk mounted, it's at
   `/etc/vakt/panel.auth` in RAM instead, and is already gone at the next
   reboot regardless.)
3. Reboot normally. The panel comes up on the first-boot setup screen again.

This is available to anyone with physical access to the console by design —
see the README's Security model section for why that's an accepted
trade-off, not an oversight.

## A service won't start or keeps crash-looping

`vakt-init` gives up restarting a service after 5 crashes in 60 seconds, and
the supervisor's status line for it will read `failed`.

1. Read its log: `cat /run/<name>.log` (and `/run/<name>.log.1` for the
   previous rotation, if the failure produced enough output to rotate).
2. Common causes:
   - **`vakt-net` failed**: usually a missing or malformed
     `/persistent/etc/vakt-net.conf`. Rewrite it from the panel's Wi-Fi Setup
     page, or check `ssid=`/`psk=`/`interface=` by hand.
   - **`vakt-ids` failed**: check `/persistent` actually mounted (`No /dev/sda
     present` in the boot log means it's running in RAM-only mode and has
     nothing to watch — not a crash, but worth knowing).
   - **A `zrpkg`-installed package's service failed**: reinstall it
     (`zrpkg remove <name> && zrpkg install <name>`) in case the install was
     interrupted or partial.
3. To force a fresh restart budget without a full reboot, there's no live
   "reset restart count" command by design (see `vakt-init/src/services.rs`)
   — reboot the appliance.

## An IDS alert fired

`vakt-ids` reports `ADDED`, `MODIFIED`, `DELETED`, or `PERMISSIONS` findings
against its SHA-256 baseline of `/persistent`.

1. Read the finding: `tail -50 /run/vakt-ids.alerts` or the panel's
   Intrusion Detection page.
2. If the change was made by you deliberately (installed a package, edited a
   config file), the baseline needs to catch up — it updates itself on the
   next scan cycle automatically, so a repeat scan without the same finding
   confirms it took.
3. If the change was **not** something you made: treat the appliance as
   compromised. This tool tells you something changed; it does not tell you
   it's safe. Reimage from a known-good build rather than trying to clean an
   appliance that's already shown signs of tampering — see
   [Recovering a compromised or badly broken appliance](#recovering-a-compromised-or-badly-broken-appliance)
   below.

## Network won't come up

1. `cat /run/vakt-net.status` — check `state`. `unconfigured` means no
   `vakt-net.conf` exists yet (expected on a fresh appliance with no Wi-Fi
   set up and no wired link). `failed` means it tried and couldn't.
2. For Wi-Fi specifically: confirm the image was built with
   `VAKT_KERNEL=host` — the `custom` kernel mode has no Wi-Fi chipset
   firmware by design (see the README's Networking section).
3. `cat /run/vakt-net.log` for `wpa_supplicant`/`udhcpc` output.

## Rotating the package-signing key

Covered in [`deploy/README.md`](../deploy/README.md#rotating-the-signing-key)
— it's a repository-side operation (regenerate the key, rebuild and reship
images with the new trust anchor), not something done on a running
appliance.

## Backing up and restoring `/persistent`

`vakt-backup` and `vakt-restore` ship in the image (`/usr/bin`, source in
[`tools/vakt-backup`](../tools/vakt-backup) and
[`tools/vakt-restore`](../tools/vakt-restore)) — no network access or extra
tooling needed, they're already on the appliance.

Run a backup periodically (by hand, or from cron/a systemd timer if you set
one up — none ships by default) and copy the result somewhere other than the
appliance itself, ideally onto a second USB drive or over the network. A
backup that lives on the same disk it's backing up survives none of the
failures worth planning for.

```bash
vakt-backup /persistent /mnt/usb/vakt-backup-$(date +%F).tar.gz
vakt-restore /mnt/usb/vakt-backup-2026-08-07.tar.gz /persistent
```

`vakt-restore` verifies the archive's `.sha256` checksum before touching
anything, refuses a truncated or corrupted backup rather than partially
restoring it, and refuses to restore into a destination that already has
files in it (unmount `/persistent` and clear it, or restore to a fresh disk,
first).

## Sending IDS alerts to a webhook (fleet setups)

If you're running more than one appliance, `vakt-ids` can POST each alert as
JSON to a URL you choose, instead of (or alongside) leaving them only in
`/run/vakt-ids.alerts` on that one machine. Off by default — a single
appliance has no need for it.

Because `vakt-init` supervises `vakt-ids` with a fixed set of startup
arguments (see `vakt-init/src/services.rs`), the URL isn't set with a flag on
a deployed appliance. Instead, write it to a one-line config file:

```bash
echo "https://your-collector.example.com/vakt-ids" > /persistent/etc/vakt-ids-webhook.conf
```

Picked up on the next `vakt-ids` start (reboot, or restart the service).
Each alert POSTs a JSON body:

```json
{"host": "appliance-3", "time": "2026-08-07T12:00:00Z", "kind": "MODIFIED", "detail": "/persistent/etc/passwd"}
```

Delivery is best-effort with a 5-second timeout: a slow or unreachable
collector logs a warning and never blocks scanning or crashes the daemon.
There's no retry — the alert file is still the durable record; the webhook
is a notification, not a queue.

## Recovering a compromised or badly broken appliance

There is deliberately no in-place "repair" path for an appliance you no
longer trust — a read-only root and mandatory package signing mean the base
system can't have been quietly modified without leaving a kernel-log or
IDS trail, but if you're not sure, don't try to reason your way to certainty
on a machine that might be lying to you.

1. Reimage: rebuild or reuse a known-good `vakt-os.iso` and write it fresh
   (`sudo dd if=vakt-os.iso of=/dev/sdX ...`).
2. Restore `/persistent` from the most recent backup made *before* the
   incident, not the appliance's current disk — copying the live data disk
   forward would carry over whatever caused the IDS alert in the first
   place.
3. Rotate the package-signing key if there's any chance it was exposed
   (it lives only on the build machine, never the appliance, so this is
   usually unnecessary — but confirm that build machine wasn't itself in
   scope of the incident).
4. Set a new panel PIN on first boot rather than restoring the old
   `vakt-panel.auth` file, if there's any chance the PIN was observed or
   guessed.
