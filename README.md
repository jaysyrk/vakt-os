# Vakt OS (`Vakt-Core`)

[Readme](README.md) | [Roadmap](ROADMAP.md) 

A secure, high-performance polyglot operating system built completely from scratch. It features a custom microkernel, safe memory and process supervisors, a native raw framebuffer UI engine, and an isolated user-space application runtime.

There is no Linux kernel, no GNU/glibc userland, and no host distribution underlying this system. It boots directly into 64-bit Long Mode on bare metal.

<html>
<pre>
GRUB (Multiboot2) → boot.S (32-to-64b transition) → kmain (Zig HAL)
                                                      │
                                                      ├── Initializes GDT, IDT, Syscall (MSR)
                                                      ├── Hands off to Memory & Task Scheduler (Rust)
                                                      └── Spawns User-Space Supervisor (Rust PID 1)
                                                            │
                                                            ├── vakt-init (Supervises userland)
                                                            ├── vakt-compositor (Direct UI drawing)
                                                            └── vakt-panel (Go TUI Application)
</pre>
</html>

## Architecture & Layers

| Layer | Component | Language | Role |
|---|---|---|---|
| **Layer 1: HAL** | `boot.S` / `main.zig` | Zig / ASM | Multiboot2 validation, 64-bit Long Mode transition, GDT/IDT management. |
| **Layer 2: Memory** | `vakt-kernel` | Rust | Buddy/Bitmap physical allocator, Slab virtual memory heap (`#![no_std]`), Preemptive Scheduler. |
| **Layer 3: IPC** | `MSR Bridge` | Zig / Rust | Maps `IA32_LSTAR` register to route `syscall` instructions into the system call array. |
| **Layer 4: Supervisor**| `vakt-init` | Rust | Freestanding PID 1 supervisor using custom `syscall!` macros instead of standard Linux headers. |
| **Layer 5: Graphics** | `vakt-compositor` | Zig / Rust | Kernel exposes a linear graphics framebuffer interface; Rust draws UI pixels via direct memory mapping. |
| **Layer 5: Userland** | `Go Runtime Port` | Go / TinyGo | Cross-compiled freestanding Unix environment with system call wrappers mapped to raw custom assembly stubs. |
| **Layer 6: Appliance** | `vakt-panel` | Go | Freestanding `tview` TUI adapted to stream inputs/outputs through custom system call vectors. |
| **Layer 6: Security** | `vakt-audit` | Go | Performance-optimized file-integrity, structural parsing, and metrics engine. |

## Building

The entire polyglot compilation workflow is orchestrated directly by Zig's build system. It coordinates the Rust bare-metal targets, the Go userland binaries, and compiles the core HAL code.

### Prerequisites
Ensure you have the Zig compiler, Rust target `x86_64-unknown-none`, and `tinygo` (or a configured Go cross-compiler) installed on your host system.

### Trigger the Orchestrator
<html>
<pre><code>
zig build
</code></pre>
</html>

This single command:
1. Compiles the Zig assembly bootstrap and hardware initialization routines.
2. Triggers `cargo build --target x86_64-unknown-none` for the core kernel, supervisor, and graphics engine.
3. Invokes `go build`/`tinygo` targeting a freestanding environment, mapping user-space apps into an isolated `RamFS` block.
4. Links all components together into a single, unified, bootable `vakt_os.iso` image.

## Running

Boot the unified image inside QEMU. Because the operating system is built from the ground up, no host kernel modules or external root filesystems are required:

<html>
<pre><code>
qemu-system-x86_64 \
    -m 2G \
    -cdrom vakt_os.iso \
    -cpu max \
    -vga std
</code></pre>
</html>

## System Internals

### 1. Hardware Abstraction & Bootstrapping (Zig/ASM)
The bootloader maps the kernel code sections strictly at the **1MB physical boundary** using a custom `linker.ld` script. The CPU transitions from 32-bit Protected Mode into 64-bit Long Mode by setting up temporary page tables, enabling PAE, and activating the LM-bit in the EFER MSR. Once in 64-bit mode, segment registers are cleared and control jumps directly into Zig's freestanding `kmain`.

### 2. Freestanding Memory & Preemptive Scheduling (Rust)
The kernel parses the Multiboot2 tags provided by the bootloader to build a Physical Page Frame Allocator. Virtual memory is managed using page tables that back a safe, `#![no_std]` heap allocator (Slab). The thread tracking system and Task Scheduler run continuously inside the kernel core, while Zig naked-assembly context-switch routines save and restore CPU registers on timer ticks.

### 3. Custom System Call Bridge (MSR)
User-space communication is achieved by programming the x86_64 `IA32_LSTAR` Model Specific Register. When a Ring 3 application calls `syscall`, the execution is routed directly into the kernel's central system call array, exposing safe primitives like `sys_write`, `sys_fork`, `sys_exec`, and `sys_ipc`.

### 4. Microkernel Service Supervisor (Rust PID 1)
`vakt-init` acts as the system's root supervisor. It uses an IPC listener to spawn, monitor, and restart background userland daemons. It tracks misbehaving processes, logs application output, and prevents broken binaries from falling into infinite crash-spin loops.

## Repository Layout

<html>
<pre>
build.zig                 Universal cross-language build orchestrator
src/hal/                  Zig entry points, boot.S, GDT, IDT initialization
src/kernel/               Rust #![no_std] memory allocators and scheduler
src/ipc/                  MSR system call router and assembly stubs
src/init/                 vakt-init freestanding supervisor (Rust)
src/compositor/           Vakt graphics framebuffer UI engine (Rust)
src/userland/panel/       Go-based tview TUI management panel
src/userland/audit/       Go-based file-integrity auditing engine
</pre>
</html>
