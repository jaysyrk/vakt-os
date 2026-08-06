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
