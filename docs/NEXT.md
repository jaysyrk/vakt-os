# What's next

[ROADMAP.md](../ROADMAP.md) is what the software does and why. This is the
working list: what to do next, in what order, and what is deliberately not
being done yet.

---

## Blocking everything else

**Wi-Fi on real hardware.** It is the only thing currently known to be broken.
`wpa_passphrase` needed a terminal a daemon does not have, so `vakt-net` now
writes the supplicant configuration itself. That fix has never run on a radio.

What the appliance has been observed to do: `/dev/rfkill` exists,
`/sys/class/ieee80211/` lists `phy0`, and `wlan0`'s `operstate` stays `down`.
So the driver and its firmware are fine and the radio is registered — the
failure is at association, with `wpa_supplicant` reporting it cannot open the
rfkill control device on every retry.

Ruled out along the way, so nobody re-treads them:

- *No chipset driver.* `phy0` is registered, and `host` is the default kernel.
- *The node appearing after the ruleset is sealed.* `load_modules()` is
  synchronous and finishes several steps before the supervisor spawns anything.
- *File permissions.* `/dev/rfkill` is `0600 root:root`, and `Service` carries
  no user field — the supervisor spawns daemons with PID 1's identity, so
  `vakt-net` and everything it runs are root.

That leaves the ruleset itself, which is why the first item below is the log
line naming what the kernel actually enforced.

- [ ] `head -5 /run/vakt-net.log` — the sandbox line says whether Landlock is
      enforced at all. Everything below assumes it is, and that is unverified
- [ ] Rebuild, boot on the machine, set up `Stkyezone_EXT` from the panel
- [ ] `cat /run/vakt-net.status` reaches `connected` with an address
- [ ] Reboot and confirm it reconnects with no prompting
- [ ] Record the chipset in [HARDWARE_VALIDATION.md](HARDWARE_VALIDATION.md) —
      it decides which firmware the image must keep carrying

Until this passes, "networking comes up" cannot be in a demo.

---

## Validation still open

Full checklist in [HARDWARE_VALIDATION.md](HARDWARE_VALIDATION.md).

| Area | State |
|---|---|
| Boot, storage, panel, PIN, read-only root | ✅ real hardware |
| IDS detection and alerting | ✅ host + booted appliance |
| Wired DHCP | ✅ QEMU — real NIC untested |
| `zrpkg` install/verify/remove | ✅ host — untested from a booted appliance |
| Wi-Fi | ❌ blocking, above |
| Secure Boot | ❌ never attempted |
| Panel Lock PIN change | ❌ writing that file as the panel's user has never succeeded anywhere |
| Framebuffer compositor | ❌ never run on a real display |
| Shutdown / poweroff | ⚠️ `poweroff` from a shell works on real hardware; the panel's `SHUTDOWN=` path is untested |
| A/B image updates | ❌ never survived one reboot |

CI now boots the image it builds — `build-system/boottest.sh` runs it headless
under QEMU over a serial console and checks it reaches a shell, seals the
root, starts both daemons, and can exec as the unprivileged user. It fails on
an image with the 0700 root that shipped, which is the point of it.

**The `host` kernel path has no CI coverage and never can** — it is defined by
matching the running kernel, so it cannot be built in a container. Every
`VAKT_KERNEL=host` change is only ever tested by someone building it on a real
Arch machine.

**Nothing verifies that the Landlock rules actually apply.** The sandbox tests
cover which paths get filtered before reaching the kernel, not enforcement:
a ruleset built in a container returns `NotEnforced`, so a probe that opens a
deliberately ungranted device node succeeds there and proves nothing. The
daemons now log any path they granted and still cannot open, which moves this
from untestable to at least observable on the appliance.

---

## Known defect: init talks over the console session

The supervisor runs on a background thread and prints with `println!`, which is
PID 1's stdout — the same `/dev/console` the panel and the shell run on. A
service reporting ready while someone is typing splices `[Vakt-Init] Service
'vakt-net' is ready.` into their command line. Observed on real hardware: a
`cat /run/vakt-net.status` turned into a "can't open" error for a path that
was never typed.

Cosmetic-looking, but it makes the console untrustworthy exactly when someone
is debugging, and every service already logs its own output to a file — this
is only the supervisor's own chatter.

The shape of the fix: a flag saying an interactive session owns the console,
set for the lifetime of each `run_on_console` child. While it is set, the
supervisor's messages append to a log instead of the console; boot output,
which happens before any session exists, is unchanged. Nothing is lost either
way, which is the property that makes it safe.

Not implemented yet on purpose: it changes PID 1's output, and it cannot be
verified without booting the image. `build-system/boottest.sh` is the right
place to assert it — a marker printed by a service after a shell prompt must
not appear on the console.

---

## Before calling anything v0.1.0

- [x] **Fix what a released ISO can do.** Done: the signed repository now
      ships as release assets from the same run that builds the ISO, so the
      keys match and `zrpkg repo <release download URL>` works. GitHub
      Releases is the repository. No hosting, still signed. Untested until
      something is actually tagged — the release path only runs on a tag.
- [ ] Wi-Fi passing, above
- [ ] Panel Lock PIN change confirmed
- [ ] Release notes: what works, what does not, which machine it was tested on
- [ ] Screenshots of the panel and a real boot
- [ ] Tag, and let CI publish the ISO

Do not tag while anything above is open. A first release that cannot install a
package is worse than no release.

---

## Getting people to actually use it

Discovery is the problem, not quality — there are hundreds of embedded-Linux
repositories and no reason for anyone to find this one.

**In order:**

1. **Topics on the repo.** Free, immediate, and the only discovery mechanism
   that works without promotion: `linux`, `operating-system`, `embedded-linux`,
   `immutable`, `security`, `appliance`, `rust`, `golang`, `zig`, `osdev`.
2. **Enable Discussions.** Somewhere for "will it run on X" that is not a bug.
3. **Social preview image.** The owl. Links get shared as cards.
4. **A 2–4 minute demo video**, once Wi-Fi works. Real machine booting,
   network coming up, a signed package installing, a **tampered** package
   being refused. The refusal is the memorable part — it is the claim that
   sounds like marketing until someone watches it happen.
   `./build-system/demo.sh --serve` stages it so every take is identical.
   *Do not demo A/B updates.* They have never worked once.
5. **Show HN**, after the video. Technical, first person: the problem, the
   architecture, what works, what does not, what feedback is wanted. Not
   "look what a teenager built" — the work is more impressive without that
   framing, and it invites the wrong kind of attention.
6. **Targeted posts**, tailored per community, not cross-posted: r/linux,
   r/rust, r/selfhosted, osdev, Lobsters if an invite turns up.
7. **Ask for hardware testers explicitly.** The
   [hardware report template](../.github/ISSUE_TEMPLATE/hardware_report.yml)
   exists for this. It is the one contribution that needs no code reading, and
   this project has been booted on exactly one machine.

**What to measure:** outside contributors, merged external PRs, hardware
reports, real bug reports. Not stars. Fifty stars with three contributors and
ten hardware reports is a much better story than two thousand drive-by stars,
and it is the difference between "built something impressive" and "ran a
project other engineers chose to join".

---

## Good first issues (drafts)

Not opened yet. Each is small, self-contained, and does not require
understanding the whole system.

| # | Issue | Where | Why it's a good entry point |
|---|---|---|---|
| 1 | **Panel: show installed packages** on the Packages page | `tools/cmd/vakt-panel` (Go) | `zrpkg list` already exists; this is wiring it into the TUI. Visible result, no new logic. |
| 2 | **Panel: wired network setup.** The only network page demands an SSID, so a wired-only machine cannot be configured from the UI at all | `tools/cmd/vakt-panel` (Go) | A real gap with an obvious shape: a form that writes `interface=` without an `ssid=`. |
| 3 | **`vakt-audit --json`** for scripting and fleet use | `tools/cmd/vakt-audit` (Go) | Pure output formatting over checks that already run. |
| 4 | **Add a `vakt-audit` check** (e.g. verify `/` really is mounted read-only) | `tools/cmd/vakt-audit` (Go) | Self-contained; the existing checks are the template. |
| 5 | **`vakt-ids`: configurable scan interval** from the config file, not just the flag | `tools/cmd/vakt-ids` (Go) | Small, and `vakt-init` supervises with fixed arguments so the flag is unreachable on an appliance. |
| 6 | **`checkimage.sh`: assert more invariants** — `/etc/passwd` present, busybox executable, `/persistent` exists | `build-system/checkimage.sh` (shell) | Shell only, and every check is one more class of unbootable image caught before boot. |
| 7 | **Architecture diagram** for the README | docs | No code at all. Genuinely wanted. |

Before opening these: label them `good first issue`, and write each one with
the file to touch, how to build it, and how to test it. An issue that says
only what to do is not a first issue.

---

## Deliberately not doing yet

- **A VPS.** One appliance on a home network does not need one — a signed repo
  can live on the build machine or on Release assets. Revisit when an appliance
  is somewhere you are not.
- **Encrypting the Wi-Fi PSK at rest.** Reasoning in [ROADMAP.md](../ROADMAP.md);
  there is no key available at boot that would not break unattended boot.
- **A second hardware architecture** (arm64). Not until x86-64 is validated on
  more than one machine.
- **Anything that makes the README claim more than has been tested.** The
  status table being honest is the most valuable thing on the page.
