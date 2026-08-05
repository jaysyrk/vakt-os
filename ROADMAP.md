## Layer 1: Security Hardening and Isolation
- [ ] **Enforce Read-Only Root Filesystem**
  - [ ] Modify build.sh to mark the rootfs partition as read-only (ro).
  - [ ] Update vakt-init to mount a volatile tmpfs over /tmp and /run during bootstrap.
- [ ] **Privilege Drop Engine**
  - [ ] Implement user and group creation (vakt user) in the base image.
  - [ ] Update vakt-init to use setuid and setgid system calls to drop root privileges before launching vakt-panel.
- [ ] **Landlock LSM Sandboxing**
  - [ ] Integrate the landlock crate into vakt-net to restrict filesystem access exclusively to /persistent/etc/vakt-net.conf.
  - [ ] Apply Landlock restrictions to vakt-compositor, blocking all file access except for /dev/fb0.

## Layer 2: Package Manager Updates (zrpkg)
- [ ] **Enforce Cryptographic Trust**
  - [ ] Remove the fallback warning behavior for unverified packages.
  - [ ] Refactor zrpkg to explicitly abort installation if an Ed25519 signature validation fails.
- [ ] **Dependency Graph Resolution**
  - [ ] Expand the package .json schema to include a dependencies string array.
  - [ ] Implement a simple Directed Acyclic Graph (DAG) solver in Rust to fetch and install prerequisites sequentially.
- [ ] **Clean Uninstallation Engine**
  - [ ] Modify zrpkg install to generate a local manifest file tracking every unpacked file path.
  - [ ] Implement zrpkg remove <name> to parse the manifest and safely delete package files.

## Layer 3: Init System and Process Supervisor (vakt-init)
- [ ] **Daemon Readiness Notifications**
  - [ ] Create a lightweight Unix domain socket mechanism inside /run/init.sock.
  - [ ] Modify background daemons to send a readiness signal (READY=1) so vakt-init knows exactly when to draw the TUI panel.
- [ ] **Graceful System Shutdown Sequence**
  - [ ] Trap SIGINT, SIGTERM, and SIGPWR in vakt-init's primary event loop.
  - [ ] Send SIGTERM to all supervised PIDs, await exit codes, sync disks, and safely unmount /persistent.
- [ ] **Supervisor Log Rotation**
  - [ ] Add a capacity clamp (e.g., 5MB limit) to the supervisor's stdout/stderr stream reader.
  - [ ] Truncate or rotate /run/<name>.log to prevent malformed or verbose daemons from exhausting volatile RAM.

## Layer 4: Infrastructure and Automation
- [ ] **Self-Contained Kernel Configuration**
  - [ ] Extract a minimal, monolithic kernel configuration (.config) stripping out unused drivers.
  - [ ] Save the configuration to build-system/kernel.config and update build.sh to compile it directly.
- [ ] **Automated CI/CD Pipeline**
  - [ ] Create .github/workflows/build.yml.
  - [ ] Configure a GitHub Actions workflow using an Arch Linux container (archlinux:latest) to build the project, run tests, and export vakt-os.iso as a release artifact.
