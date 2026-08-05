##*Making full kernal*
## Layer 1: Hardware Abstraction & Bootstrapping (HAL)
- [ ] **Multiboot2 Entry & Long Mode Setup**
  - [ ] **[ZIG]** Write the assembly stub and aligned Multiboot2 header to transition the CPU from 32-bit to 64-bit Long Mode.
  - [ ] Create a `linker.ld` script to map kernel code sections strictly at the 1MB physical boundary.
- [ ] **CPU Tables & Interrupt Handling**
  - [ ] **[ZIG]** Initialize the Global Descriptor Table (GDT) setting up privilege rings (Ring 0 for Kernel, Ring 3 for Userland).
  - [ ] **[ZIG]** Configure the Interrupt Descriptor Table (IDT) to capture hardware ticks and keyboard presses.

## Layer 2: Memory & Process Management
- [ ] **Freestanding Memory Allocators**
  - [ ] **[ZIG]** Parse Multiboot2 tags to build a Physical Page Frame Allocator (Buddy/Bitmap).
  - [ ] **[RUST]** Implement page tables for virtual memory allocation and back a `#![no_std]` heap allocator (Slab).
- [ ] **Preemptive Scheduler**
  - [ ] **[RUST]** Implement a thread tracking system and a Task Scheduler inside the kernel core.
  - [ ] **[ZIG]** Write the naked-assembly context-switch routines to save and restore CPU registers on timer ticks.

## Layer 3: System Calls & Kernel IPC (The Bridge)
- [ ] **MSR Syscall Interface**
  - [ ] **[ZIG]** Program `IA32_LSTAR` to route the x86_64 `syscall` instruction cleanly from Ring 3 into the kernel.
  - [ ] **[RUST]** Map raw registers into an internal system call array (`sys_write`, `sys_fork`, `sys_exec`, `sys_ipc`).

## Layer 4: Custom Init System & Supervisor
- [ ] **Porting vakt-init to Vakt-Core**
  - [ ] **[RUST]** Rewrite `vakt-init` using standard `#![no_std]` without any Linux headers or `glibc` dependencies.
  - [ ] **[RUST]** Implement custom `syscall!` wrapper macros to substitute standard Linux kernel communication.
- [ ] **Microkernel Service Supervisor**
  - [ ] **[RUST]** Establish an IPC listener that spawns, monitors, and restarts your background userland daemons.

## Layer 5: Graphics & Storage Porting
- [ ] **Vakt Framebuffer UI Engine**
  - [ ] **[ZIG]** Expose a simple linear graphics framebuffer interface from the kernel.
  - [ ] **[RUST]** Update `vakt-compositor` to draw UI pixels directly to this kernel-allocated video memory block.
- [ ] **The Go Runtime User-Space Compatibility Port**
  - [ ] **[GO]** Configure Go to compile targeting an entirely freestanding Unix environment (cross-compile via `TinyGo` or customized targets to completely strip out host OS expectations).
  - [ ] **[GO]** Map Go's low-level system call wrappers to call your custom `sys_write` and `sys_ipc` assembly stubs instead of standard Linux system calls.

## Layer 6: High-Level Appliance Applications
- [ ] **The Vakt Panel UI (vakt-panel)**
  - [ ] **[GO]** Adapt your core appliance TUI management panel (`tview`) to stream inputs and outputs through your custom system call vectors.
- [ ] **Security Auditing Engine (vakt-audit)**
  - [ ] **[GO]** Write the file-integrity and metric logging programs in Go, utilizing Go's speed for structural parsing and networking.

## Layer 7: Cross-Language Build Infrastructure
- [ ] **The Universal build.zig Orchestrator**
  - [ ] **[ZIG]** Configure `zig build` to build the Zig hardware initialization.
  - [ ] **[ZIG]** Trigger `cargo build --target x86_64-unknown-none` for the Kernel Core, `vakt-init`, and `vakt-compositor`.
  - [ ] **[ZIG]** Invoke `go build` / `tinygo` to generate static userland assets, mapping them directly into an isolated RamFS block.
  - [ ] Link them all together into a final, unified bootable image (`vakt_os.iso`).
