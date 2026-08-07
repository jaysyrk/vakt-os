# Contributing to Vakt OS

[Readme](README.md) | [Roadmap](ROADMAP.md) | [Contributing](CONTRIBUTING.md)

## What this project is

An operating system built from scratch, deliberately. When a decision comes up
between "pull in a crate that does this" and "write the fifty lines it takes",
this project writes the fifty lines — that is the point of it, not an oversight.
Dependencies that are genuinely infrastructure (a syscall wrapper, a crypto
implementation, a TUI toolkit) are fine; dependencies that replace something
worth understanding are not.

Anything that does come in from outside gets recorded in the README's
third-party section, with a link to where it came from.

## Getting set up

You need a Rust toolchain, Go, Zig 0.16, and — for building an image — an Arch
host with root. The code builds and the tests run anywhere; only `build.sh` is
Arch-specific.

Zig's standard library moves fast between versions - `vakt-verify` is written
against 0.16 specifically (its process/IO entry-point convention changed
again as recently as this release), so a much older or newer compiler may not
build it. Arch's `extra/zig` package tracks this.

```bash
cargo test --manifest-path vakt-init/Cargo.toml
cargo test --manifest-path pkg-manager/Cargo.toml
cargo test --manifest-path vakt-net/Cargo.toml
cd tools && go test ./cmd/...
cd vakt-verify && zig build test
```

Building an image, which is the only way to test the parts that need to be
PID 1:

```bash
sudo ./build.sh                       # default (VAKT_KERNEL=host): fast, reuses the host kernel
sudo VAKT_KERNEL=custom ./build.sh    # slow: builds the monolithic kernel from scratch
```

Then boot it with the QEMU command the build prints at the end.

### Fuzzing

`pkg-manager/fuzz/` has `cargo-fuzz` targets for the parsers that handle
untrusted, network-sourced input before anything trusts it - archive path
validation, manifest JSON, and signature verification. Requires a nightly
toolchain and `cargo install cargo-fuzz`; not part of default CI. See
[`docs/SECURITY_AUDIT.md`](docs/SECURITY_AUDIT.md) for what each target
covers and why.

```bash
cd pkg-manager && cargo +nightly fuzz run safe_relative
```

## Before you open a pull request

CI runs all of this, so running it first saves a round trip:

```bash
for c in pkg-manager vakt-init vakt-net vakt-compositor; do
    cargo fmt --manifest-path $c/Cargo.toml --check
done
gofmt -l tools            # must print nothing
cd tools && go vet ./...
(cd vakt-verify && zig fmt --check src/main.zig build.zig)
bash -n build.sh build-system/*.sh deploy/*.sh
```

## Writing code here

**Tests are for behaviour that would otherwise be a guess.** The supervisor is
tested for what happens when a daemon crash-loops, when a binary is missing, and
when a foreground child's exit code could be stolen by a background reaper. The
package manager is tested for dependency cycles, for archives that try to escape
the install root, and for removal refusing to follow a symlink. A test that only
restates the implementation is not worth its maintenance; a test that pins down
something subtle is worth several.

**Comments explain the decision, not the mechanism.** `// increment the
counter` is noise. "This reaps per-PID rather than with `waitpid(-1)` because
the main thread is collecting the panel's exit code and a wildcard reaper would
race it" is the comment that stops someone helpfully simplifying it back into a
bug. If a piece of code looks like it could be simpler, say why it is not.

**Failure modes are part of the design.** Everything here runs unattended on a
machine with no operator: decide what happens when the disk is missing, when the
network never comes up, when a daemon will not die. "It cannot happen" is not a
plan.

**Nothing widens the security model quietly.** The read-only root, the
unprivileged panel, the Landlock rulesets, mandatory package signatures, and
the independent Zig re-verification of those signatures are the five things
this project exists to demonstrate. A change that punches through one of them
needs to say so plainly in the pull request, and needs a reason better than
convenience.

**A from-scratch second implementation only earns its place if it stays
independent.** `vakt-verify` exists to catch a mistake `zrpkg` alone would
not: it must never import `zrpkg`'s code, read its intermediate state, or
otherwise become a second copy of the same logic wearing a different
language. If you touch one, ask whether the other still constitutes a
genuinely separate check.

## Where things live

| Path | What it is |
|---|---|
| `vakt-init/` | PID 1: mounting, sealing the root, privilege drop, supervision, readiness, shutdown |
| `pkg-manager/` | `zrpkg`: dependency resolution, signature verification, install and removal |
| `vakt-net/` | The networking daemon and its Landlock sandbox |
| `vakt-compositor/` | Framebuffer rendering |
| `vakt-verify/` | Independent Ed25519/SHA-256 package signature verifier (Zig) |
| `tools/cmd/` | The Go tools: panel, audit, IDS, repository server |
| `tools/vakt-backup`, `tools/vakt-restore` | Data-disk backup/restore, POSIX sh, ships in the image |
| `build-system/` | Kernel configuration and builder, repository builder, logo |
| `deploy/` | Running the repository on a rented server: systemd unit, publish script |
| `docs/OPERATIONS.md` | Operator runbook: lockouts, crash loops, IDS alerts, backups |
| `build.sh` | Assembles the rootfs and the ISO |

## Adding a package to the repository

`build-system/mkrepo.sh` has a `PACKAGES` array. Each line is
`name|binary|version|description|dependencies`, and the dependency field is a
comma-separated list that may be empty. `zrpkg` resolves the graph itself, so
only direct dependencies need listing.

## Adding a supervised service

`DEFAULT_SERVICES` in `vakt-init/src/services.rs`. Set `notifies: true` only if
the daemon actually sends `READY=1` to `$NOTIFY_SOCKET` — boot waits for the
ones that claim they will, and a service that claims it and does not costs every
boot the full readiness timeout. `vakt-net/src/notify.rs` and
`tools/cmd/vakt-ids/notify.go` are the two existing clients; either is a
complete example.

## Changing the kernel configuration

Symbols above the `OPTIONAL` marker in `build-system/kernel.config` are required:
the build fails if one does not survive configuration. Put anything boot-critical
or security-critical there. Symbols below it are best-effort, for options whose
names change between kernel releases.

Adding a Wi-Fi chipset driver means adding both its `CONFIG_` symbol and its
firmware, which the monolithic image does not otherwise carry.

## Commits

Present tense, describing the change and, where it is not obvious, why.
Keep formatting-only changes out of commits that change behaviour.
