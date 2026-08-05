## Layer 1: Security Hardening and Isolation
- [ ] **Enforce Read-Only Root Filesystem**
  - [ ] Modify build.sh to mark the rootfs partition as read-only (ro).
  - [ ] Update vakt-init to mount a volatile tmpfs over /tmp and /run during bootstrap.
- [ ] **Privilege Drop Engine**
  - [ ] Implement user and group creation (vakt user) in the base image.
  - [ ] Update vakt-init to use setuid and setgid system calls to drop root privileges before launching vakt-panel.
- [ ] **Landlock LSM Sandboxing (Zig & Rust implementation)**
  - [ ] Integrate the landlock crate into vakt-net to restrict filesystem access exclusively to /persistent/etc/vakt-net.conf.
  - [ ] **[ZIG]** Implement a lightweight Landlock sandboxing C-ABI library in Zig (`libvakt_sandbox.a`) using raw `linux_syscalls` to block all file access except for `/dev/fb0`. Link this directly into `vakt-compositor` (Rust).

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
- [ ] **Supervisor Log Rotation (Zig Shared Engine)**
  - [ ] **[ZIG]** Build a freestanding, zero-allocation log streaming clamp engine (`vakt-rotator`) in Zig. 
  - [ ] Embed the Zig log rotator engine directly into `vakt-init` via FFI to truncate or rotate `/run/<name>.log` when it hits a 5MB capacity clamp, preventing volatile RAM exhaustion.

## Layer 4: Infrastructure and Automation
- [ ] **Self-Contained Kernel Configuration**
  - [ ] Extract a minimal, monolithic kernel configuration (.config) stripping out unused drivers.
  - [ ] Save the configuration to build-system/kernel.config and update build.sh to compile it directly.
- [ ] **Unified Polyglot Build Orchestration**
  - [ ] **[ZIG]** Replace the host-dependent, pacman-locked bash logic with a root-level `build.zig` master script.
  - [ ] Configure `zig build` to cross-compile the kernel config, invoke `cargo` via `std.ChildProcess` for the Rust components, compile the Go panel tools, and package the static `vakt-os.iso`.
- [ ] **Automated CI/CD Pipeline**
  - [ ] Create .github/workflows/build.yml.
  - [ ] Configure a GitHub Actions workflow that provisions the Zig toolchain, builds the entire multi-language stack via `zig build`, runs tests, and exports the final ISO as a release artifact.
