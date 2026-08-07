## Layer 1: Security Hardening and Isolation
- [x] **Enforce Read-Only Root Filesystem**
  - [x] Modify build.sh to mark the rootfs partition as read-only (ro).
  - [x] Update vakt-init to mount a volatile tmpfs over /tmp and /run during bootstrap.
- [x] **Privilege Drop Engine**
  - [x] Implement user and group creation (vakt user) in the base image.
  - [x] Update vakt-init to use setuid and setgid system calls to drop root privileges before launching vakt-panel.
- [x] **Landlock LSM Sandboxing**
  - [x] Integrate the landlock crate into vakt-net to restrict filesystem access exclusively to /persistent/etc/vakt-net.conf.
  - [x] Apply Landlock restrictions to vakt-compositor, blocking all file access except for /dev/fb0.

## Layer 2: Package Manager Updates (zrpkg)
- [x] **Enforce Cryptographic Trust**
  - [x] Remove the fallback warning behavior for unverified packages.
  - [x] Refactor zrpkg to explicitly abort installation if an Ed25519 signature validation fails.
- [x] **Dependency Graph Resolution**
  - [x] Expand the package .json schema to include a dependencies string array.
  - [x] Implement a simple Directed Acyclic Graph (DAG) solver in Rust to fetch and install prerequisites sequentially.
- [x] **Clean Uninstallation Engine**
  - [x] Modify zrpkg install to generate a local manifest file tracking every unpacked file path.
  - [x] Implement zrpkg remove <name> to parse the manifest and safely delete package files.

## Layer 3: Init System and Process Supervisor (vakt-init)
- [x] **Daemon Readiness Notifications**
  - [x] Create a lightweight Unix domain socket mechanism inside /run/init.sock.
  - [x] Modify background daemons to send a readiness signal (READY=1) so vakt-init knows exactly when to draw the TUI panel.
- [x] **Graceful System Shutdown Sequence**
  - [x] Trap SIGINT, SIGTERM, and SIGPWR in vakt-init's primary event loop.
  - [x] Send SIGTERM to all supervised PIDs, await exit codes, sync disks, and safely unmount /persistent.
- [x] **Supervisor Log Rotation**
  - [x] Add a capacity clamp (e.g., 5MB limit) to the supervisor's stdout/stderr stream reader.
  - [x] Truncate or rotate /run/<name>.log to prevent malformed or verbose daemons from exhausting volatile RAM.

## Layer 4: Infrastructure and Automation
- [x] **Self-Contained Kernel Configuration**
  - [x] Extract a minimal, monolithic kernel configuration (.config) stripping out unused drivers.
  - [x] Save the configuration to build-system/kernel.config and update build.sh to compile it directly.
- [x] **Automated CI/CD Pipeline**
  - [x] Create .github/workflows/build.yml.
  - [x] Configure a GitHub Actions workflow using an Arch Linux container (archlinux:latest) to build the project, run tests, and export vakt-os.iso as a release artifact.

## Layer 5: Production Readiness
- [x] **Panel Authentication**
  - [x] Add a salted, hashed PIN gate vakt-panel requires before showing the main menu.
  - [x] First-boot setup screen (with an explicit, explained skip), and a Panel Lock page to change or remove the PIN afterward.
- [x] **Boot-Time Kernel Hardening**
  - [x] Apply a fixed set of hardening sysctls (ptrace_scope, kptr_restrict, dmesg_restrict, rp_filter, and related network sysctls) in vakt-init, best-effort so a kernel built without a given knob does not fail to boot.
- [x] **Real vakt-audit Checks**
  - [x] Parse /etc/passwd for UID 0 exclusivity instead of only checking that a lookup succeeds.
  - [x] Replace the mocked sysctl check with real /proc/sys reads against the same list vakt-init hardens.
- [x] **zrpkg-server Rate Limiting**
  - [x] Per-IP token bucket (-rate-limit/-rate-burst), rejecting with 429 before any file is touched.
- [ ] **OS Image Update Mechanism** — deliberately deferred, see notes below.
  - [ ] A way to update the base image (kernel, init, core tools) in the field, not just zrpkg packages.
  - [ ] A/B partitions or another rollback path so a bad update cannot brick a deployed appliance.
- [ ] **Encrypted Wi-Fi Credentials at Rest** — decided against, see notes below.
  - [ ] Something stronger than root-only file permissions for the PSK in /persistent/etc/vakt-net.conf.
- [x] **Fleet Observability**
  - [x] Ship logs, metrics or alerts off the device - useful for anyone running more than one appliance.
- [x] **Disaster Recovery for /persistent**
  - [x] A backup or snapshot path for the data disk; right now corruption there is simply unrecoverable.
- [x] **Security Audit**
  - [x] Fuzzing and a focused review of every `unsafe` Rust block (privilege drop, mmap, ioctl).
  - [ ] Independent/third-party review before this runs anything that matters. *(out of scope for a solo project - needs an outside reviewer.)*
- [x] **Operator Documentation**
  - [x] An incident-response runbook, a key-rotation procedure, and steps for recovering a bricked appliance.
- [ ] **Real Hardware Validation** — deliberately deferred, see notes below.
  - [ ] CI only proves the ISO builds and boots in a container/QEMU; physical NICs, Wi-Fi chipsets, storage controllers and Secure Boot still need testing on real machines.

---

### Notes on what shipped

A few decisions worth recording, where the implementation is narrower or wider
than the line above it. Details are in the [README](README.md).

**Landlock on vakt-net.** Restricting the daemon to *only* its configuration
file would stop it working: it drives `ip`, `wpa_supplicant` and `udhcpc`, and a
Landlock ruleset is inherited by everything it spawns. What ships is the
tightest ruleset that leaves it functional — read and execute on the image's
program directories, read/write on `/run`, read on `/proc` and `/sys` — with the
configuration file as the only reachable path under `/persistent`. The rest of
the data disk is closed to the one daemon that talks to the network, which is
the property the line was after.

**Signals.** `SIGUSR1` and `SIGUSR2` are handled alongside the three named
signals, because that is what busybox's `halt` and `poweroff` send to PID 1.
Without them the shutdown sequence would exist but be unreachable from a shell.

**Shutdown from the panel.** The panel now runs unprivileged and so cannot
signal PID 1. `/run/init.sock` therefore accepts `SHUTDOWN=poweroff|reboot|halt`
as well as readiness, and the socket is `root:vakt` mode 0660 so only init's own
group can ask. Otherwise the graceful shutdown would be unreachable in normal
operation.

**Kernel configuration.** `build-system/kernel.config` is a seed fed to
`make allnoconfig KCONFIG_ALLCONFIG=…` rather than a committed 5,000-line
`.config`. It produces the same monolithic result, stays readable, and survives
a kernel version bump. `build-system/mkkernel.sh` verifies afterwards that every
required symbol actually survived, and fails the build listing any that did not.

**Kernel modes.** `VAKT_KERNEL=host` remains supported. The monolithic kernel
carries no Wi-Fi chipset drivers or firmware, so hardware it has no driver for
still needs the host kernel's modules.

**Panel PIN.** There is no server-side identity here to authenticate against -
it is one hashed value on the data disk, checked against what the person at
the console typed. That is deliberately weak compared to a real login system;
what it defends against is exactly one thing, someone with physical access
opening the panel, and the existing `vakt.rootshell` recovery path is the
answer to a forgotten PIN, not a new hole - it already required console access
to use.

**Sysctl hardening and the audit check.** `vakt-init/src/sysctl.rs` and
`tools/cmd/vakt-audit/main.go` carry the same list of paths and values by
hand, not by import - `vakt-audit` has to read the real state of the system to
mean anything, and importing the setter's list would make it check that the
setter ran, not that hardening exists. This is the same independence argument
`vakt-verify` makes for signatures, applied to a smaller thing.

**Rate limiting is per-process, not shared.** The token buckets in
`zrpkg-server` live in memory. Running more than one instance behind a load
balancer would give each instance its own budget per IP rather than one
shared budget - fine for the single-server deployment this project documents,
worth knowing before scaling past it.

**Fleet observability is a webhook, not a metrics pipeline.** `vakt-ids` can
POST each alert as JSON to an operator-chosen URL
([docs/OPERATIONS.md](docs/OPERATIONS.md#sending-ids-alerts-to-a-webhook-fleet-setups)),
best-effort with a 5-second timeout and no retry - the alert file stays the
durable record. Deliberately the smallest thing that gets alerts off a single
box: a real metrics/logs pipeline (Prometheus exposition, structured log
shipping) is a much bigger surface and wasn't what "useful for anyone running
more than one appliance" required.

**Backup/restore covers `/persistent`, not the base image.** `vakt-backup`
and `vakt-restore` ([docs/OPERATIONS.md](docs/OPERATIONS.md#backing-up-and-restoring-persistent))
handle the one thing that's actually irreplaceable on a running appliance -
the data disk. The base image is already reproducible from this repository,
so it never needed a backup path; that's also why OS image updates (below)
are a separate, harder problem from this one.

**Security audit found and fixed one real bug.** The unsafe-block review
turned up a genuine data race between parallel tests sharing a process-global
environment variable, not just a documentation pass - see
[docs/SECURITY_AUDIT.md](docs/SECURITY_AUDIT.md) for the finding, the fix,
and the three new `cargo-fuzz` targets covering `zrpkg`'s untrusted-input
parsing. The one box deliberately left unchecked is independent/third-party
review, which by definition can't be done by the same person who wrote the
code.

**Why three items are still deferred, not silently dropped:**

- **OS image update mechanism.** This needs A/B partitions or an equivalent
  rollback path, which is a boot/partition-layout redesign - not something
  to implement without the ability to actually boot-test a bad update and
  confirm the rollback recovers, which this environment cannot do. Shipping
  something untested here would be worse than leaving the gap documented:
  a fake rollback path is more dangerous than no rollback path, because it
  invites trust it hasn't earned.
- **Encrypting the Wi-Fi PSK at rest.** Decided against, not just deferred.
  The appliance is designed to boot and connect to the network unattended,
  with no prompts - that's the whole point of
  `/persistent/etc/vakt-net.conf` being readable at boot before anyone is at
  the console. The only real secret available on the device, the panel PIN,
  doesn't exist yet at that point in boot: it's entered later, after the
  network already needed the PSK. Any encryption scheme needs a key
  available before boot-time network bring-up; the realistic option (a
  TPM-sealed key, unsealed automatically at boot) adds a hardware dependency
  and a new unseal-failure mode without changing the actual threat model -
  physical console access already gets a root shell via `vakt.rootshell`,
  the same trade-off this would be protecting against. Root-only file
  permissions stay, and the reasoning is now explicit in the README's
  Security model section rather than an unexplained gap.
- **Real hardware validation.** CI proves the ISO builds and boots under
  QEMU. Physical NICs, Wi-Fi chipsets, storage controllers, and Secure Boot
  behavior can only be validated on real machines, which this environment
  does not have access to. This is the one item on the list that isn't a
  design question at all - it just needs someone with the hardware to run
  the ISO and report back. [docs/HARDWARE_VALIDATION.md](docs/HARDWARE_VALIDATION.md)
  is the checklist for that pass, covering Secure Boot, boot sequence,
  storage controller detection, wired/Wi-Fi networking, panel input,
  package install, IDS alerting, and shutdown - not just "did it boot."
