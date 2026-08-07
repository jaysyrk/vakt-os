# OS Image A/B Updates

A way to update the kernel, `vakt-init`, and every other tool baked into the
image in the field, without re-flashing the boot media by hand - with an
automatic rollback if the new image never reaches a working state.

**Status: implemented, unvalidated on real boot hardware.** Every piece that
can be tested without an actual reboot has been - the decision logic is unit
tested, the GRUB config passes `grub-script-check`, and CI actually builds a
signed update bundle and runs `vakt-update apply` against a local repository
end to end. What none of that can confirm: whether GRUB really boots slot B
the way this assumes, and whether vakt-init's rollback really recovers a
machine stuck on a bad one. That needs a real reboot on real hardware - see
[HARDWARE_VALIDATION.md](HARDWARE_VALIDATION.md#os-image-ab-updates). Until
that has happened at least once, treat this as "should work, budget for it
being wrong" rather than "works."

## Why this design

The obvious A/B approach - two rootfs partitions on the boot medium, GRUB
picking between them - doesn't fit how this appliance actually boots. There
is no rootfs partition: the whole OS is an initramfs (`boot/initramfs.img`),
loaded from a `grub-mkrescue` ISO9660 image that's `dd`'d onto a USB stick.
Making that medium writable and repartitioning it would touch the one thing
in this project that's actually been built and tested - the ordinary install
path - for the sake of a feature most appliances will use rarely.

Instead:

- **Slot A is the boot medium, unchanged.** Whatever was `dd`'d to the USB.
  Nothing ever writes to it again. It is the one thing an update can never
  corrupt, which is also why it's the only safe rollback target.
- **Slot B lives on `/persistent`** - the data disk every appliance already
  has, already writable, already backed up by `vakt-backup`. An update is
  just another thing that lives there, the same trust boundary as an
  installed `zrpkg` package.
- **GRUB decides which to boot by reading a variable off `/persistent`**,
  not by anything vakt-init tells it - GRUB runs before vakt-init exists.
- **vakt-init decides whether to trust what GRUB just booted**, and can
  overrule it on the *next* boot if this one doesn't pan out.

This means the riskiest part of a from-scratch A/B design - a writable
bootloader partition that could itself get corrupted - doesn't exist here.
Worst case, `/persistent` is unreadable or the update state is nonsense, and
GRUB's own logic (see below) falls back to slot A on its own, no vakt-init
required.

## The pieces

| Piece | Lives in | Job |
|---|---|---|
| `vakt-update` | `vakt-update/` (new crate) | Operator-run CLI: fetch, verify, stage an update onto `/persistent`. |
| `vakt-init/src/update.rs` | `vakt-init/` | Boot-time decision: trust this slot, or roll back. |
| `vakt-init/src/envblock.rs`, `vakt-update/src/envblock.rs` | both crates (duplicated - see below) | Reads/writes the GRUB environment block both sides agree through. |
| `build-system/mkupdate.sh` | `build-system/` | Builds and signs a slot-B bundle, the update-side counterpart to `mkrepo.sh`. |
| grub.cfg's slot-B stanza | `build.sh` (the heredoc) | Reads the same environment block, boots slot B if it's active and present. |

### Reusing zrpkg instead of a second implementation

An update bundle is fetched, verified, and unpacked exactly like a `zrpkg`
package - same `download_package`, same `verify_signature`, same
`PackageManifest`, same `safe_relative` archive-path check, same repository
URL resolution (`zrpkg::config::load`), same trust anchor
(`/etc/vakt/trusted.key`), same signing command (`zrpkg pack`). `vakt-update`
depends on `pkg-manager` as a library (`pkg-manager/src/lib.rs`, extended
this pass to expose `config` alongside the modules already there for
fuzzing) rather than reimplementing any of it.

This matters beyond avoiding duplicate code: `verify_signature` and
`safe_relative` are exactly the two functions `pkg-manager/fuzz/` already
fuzzes (see [SECURITY_AUDIT.md](SECURITY_AUDIT.md)). Reusing them means the
update path inherits that fuzzing instead of being a second, unfuzzed
implementation of the same untrusted-input handling - an update bundle is
not a lower-stakes thing to get wrong than a package.

### The GRUB environment block

GRUB's own mechanism for a bootloader-readable variable that survives a
reboot: a fixed 1024-byte file (`# GRUB Environment Block\n` followed by
`key=value\n` lines, padded with `#`), normally managed by `grub-editenv`.
Reimplemented from the documented format in `envblock.rs` - once in each of
`vakt-init` and `vakt-update`, since they're separate crates and the format
is small (about 40 lines) - rather than shipping the `grub-editenv` binary
itself into the read-only initramfs for one file format.

Stored at `/persistent/etc/vakt/bootenv`. The only key either side writes is
`vakt_active` (`A` or unset means slot A; `B` means slot B).

**Unvalidated specifically:** the signature and padding behavior are
reimplemented from GRUB's documented source, and the write/read round-trips
correctly under Rust's own unit tests - but no actual `grub` binary has ever
read a block this code wrote. `grub-script-check` (run in CI, see below)
confirms the *config* that reads it is syntactically valid GRUB script; it
does not run the script, so it cannot confirm `load_env` actually parses this
block the way assumed here.

### The boot decision (GRUB side)

grub.cfg (written by `build.sh`, unchanged by an update - only what's in
`/persistent` changes):

1. `search --no-floppy --label VAKTDATA --set=vakt_data` - finds the data
   disk by filesystem label (`build.sh` now labels `vakt-data.img` and the
   README's real-hardware `mkfs.ext4` instructions were updated to match),
   since GRUB's device names don't match the kernel's. Left unset, not
   fatal, if the disk isn't present or isn't labeled - a fresh appliance
   with no data disk yet still boots slot A.
2. `load_env -f ($vakt_data)/etc/vakt/bootenv` if the disk was found. A
   missing file is a no-op, leaving `$vakt_active` unset.
3. If `$vakt_active` is `B` *and* `($vakt_data)/vakt-update/B/vmlinuz`
   actually exists, boot menu entry 2 (slot B). Otherwise, entry 0 (slot A).

Every ambiguous case - no data disk, no bootenv, `vakt_active` unset, slot B
files missing - resolves to slot A. GRUB never has to decide slot B is
*good*, only that it's *present and requested*; whether it's actually good
is vakt-init's job, one layer up, because that decision needs things GRUB
can't evaluate (did the daemons start, did the panel come up).

### The boot decision (vakt-init side)

`vakt-init/src/update.rs`, called twice from `main()`:

- **Early**, right after `/persistent` is mounted, before any service
  starts: reads `/etc/vakt-slot` (baked into each slot's own initramfs -
  `A` for the image `build.sh` produces normally, `B` for what
  `mkupdate.sh` produces) and `/persistent/etc/vakt-update-state.json`
  (`{"tries_left": N}`, written by `vakt-update apply`). The actual
  decision (`next()` in that file) is a pure function, fully unit tested:

  | Slot | State file | Outcome |
  |---|---|---|
  | A | (any) | Nothing to do - slot A is never rolled back from. |
  | B | missing | Nothing to do - either never updated, or already confirmed. |
  | B | `tries_left > 0` | Decrement and keep booting. |
  | B | `tries_left == 0` | Roll back: write `vakt_active=A`, delete the state file, `reboot(2)` immediately. |

  A rollback here never returns - there is no supervisor running yet to
  stop gracefully, so a direct `reboot(2)` is correct, not the graceful
  shutdown sequence that exists for a running system.

- **At the point boot already considers itself successful** - right before
  the panel is drawn, the same point that decides whether to keep waiting
  for service readiness - `confirm()` deletes the state file if this is
  slot B. From then on this boot of slot B is permanent until the next
  update; there is no ongoing "N successful boots in a row" requirement,
  just "reached this point at least once."

### The operator side (`vakt-update`)

```bash
vakt-update check              # what's staged, what's available, from where
vakt-update apply [--reboot]   # fetch, verify, stage; optionally reboot now
```

`apply` downloads the bundle, verifies its signature, extracts into a
staging directory, checks `vmlinuz`/`initramfs.img` are both present (refuses
a partial bundle rather than activating one), then atomically renames the
staging directory into `/persistent/vakt-update/B/`, writes
`vakt_active=B` to the bootenv, and writes the state file with
`tries_left = 3`. Nothing about this is automatic or scheduled - matches the
project's existing "no automatic mystery actions" stance (`zrpkg install` is
the same way).

### Building and publishing an update (`mkupdate.sh`)

```bash
sudo ./build-system/mkupdate.sh [version]
```

Runs `build.sh` itself with `VAKT_SLOT=B` and `VAKT_UPDATE_OUT` set, which
makes `build.sh` stop right after producing `vmlinuz`/`initramfs.img` -
skipping the ISO and data-disk steps, which an update bundle doesn't need -
and copies those two files out. Signs the result with `zrpkg pack`, using
the same `build-system/keys/repo.key` `mkrepo.sh` uses, and publishes
`vakt-update.zrp`/`.json` into `tools/repo/` beside the ordinary packages.
There is deliberately no separate update-server concept - `vakt-update`
fetches from the same repository URL as `zrpkg` (`zrpkg::config::load`).

## What's genuinely verified, and what isn't

**Verified, this pass:**
- The decision logic in `update.rs` (`next()`) - unit tested for every
  slot/state combination.
- The GRUB environment block format - unit tested round-trip, in both
  crates.
- grub.cfg's syntax - `grub-script-check` (real GRUB tooling, installed in
  CI specifically for this) parses it without error. This checks syntax,
  not behavior.
- The whole `vakt-update` pipeline except GRUB and reboot - CI actually
  builds a real slot-B image with `mkupdate.sh`, signs it, serves it from a
  real `zrpkg-server`, and runs `vakt-update apply` against it for real,
  then asserts the expected files and bootenv contents exist afterward.

**Not verified, and can't be from here:**
- Whether GRUB actually finds `/persistent` via `search --label VAKTDATA`
  on real firmware.
- Whether `load_env` actually parses a bootenv this code wrote.
- Whether GRUB actually boots slot B's `vmlinuz`/`initramfs.img` from the
  data disk correctly.
- Whether a rollback (`vakt-init` writing `vakt_active=A` and calling
  `reboot(2)`) actually results in GRUB booting slot A on the next attempt.
- Whether three failed boot attempts is the right number in practice, as
  opposed to a reasonable-sounding guess.

Every one of those needs an actual reboot, which needs real hardware (or at
minimum QEMU, neither of which was available while this was built - see
[HARDWARE_VALIDATION.md](HARDWARE_VALIDATION.md#os-image-ab-updates) for the
specific test sequence to run before trusting this on an appliance anyone
depends on).
