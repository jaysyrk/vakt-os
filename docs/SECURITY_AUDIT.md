# Security Audit Notes

A from-scratch review pass: every `unsafe` Rust block read and re-justified
against what it actually needs to be sound, plus new fuzzing infrastructure
for the parsers that handle untrusted, network-sourced input. Not a
substitute for the independent/third-party review still listed as open in
[ROADMAP.md](../ROADMAP.md) — this is what one more pass from inside the
project could reasonably find and fix.

## Finding: a real data race between parallel tests

Two pairs of tests — `pkg-manager/src/config.rs`'s `REPO_URL_ENV` tests and
`pkg-manager/src/install.rs`'s `ZRPKG_PUBKEY` tests — set or read the same
process-global environment variable, and `cargo test` runs tests in the same
binary on separate threads by default. `env::set_var`/`remove_var` were
marked `unsafe` in recent Rust specifically because of this: a set on one
thread and a read on another can race at the libc level, not just logically.

This wasn't hypothetical here. `config.rs` had three tests touching
`REPO_URL_ENV` — one setting it, two others calling `load_from` while
expecting it *unset* — that could genuinely observe each other's writes if
scheduled concurrently, occasionally producing a spurious pass or fail
depending on timing. `install.rs` had the equivalent pair for `ZRPKG_PUBKEY`.

**Fixed** with a `static Mutex<()>` per file that every test touching the
variable takes first, serializing just those tests against each other
without affecting the rest of the suite's parallelism. Verified by running
the full suite 20 times with `--test-threads=8` (rather than the sequential
`--test-threads=1` cargo already tends toward on lightly loaded machines)
before and after — the race was schedule-dependent enough that a handful of
runs wasn't a reliable reproduction either way, but the fix addresses the
actual documented hazard (`unsafe` on these functions exists for exactly
this reason), not a guess.

## Unsafe block review

Every `unsafe` block outside test code, as of this pass:

| Location | What it does | Verdict |
|---|---|---|
| `vakt-init/src/main.rs:46` | `env::set_var` for `PATH`/`HOME` | Sound — the first statements in `main()`, before any thread exists. |
| `vakt-init/src/main.rs:112` | `env::set_var` for the readiness socket path | Sound — still single-threaded; all `thread::spawn` calls happen later in `main()`. |
| `vakt-init/src/main.rs:216` | `Command::pre_exec` running `privilege::become_user` | Sound — runs in the forked child between `fork` and `exec`, sticks to async-signal-safe `nix` syscalls only, allocates nothing. Already documented in place. |
| `vakt-compositor/src/main.rs:95` | Two `ioctl` calls reading framebuffer geometry | Sound — the `#[repr(C)]` structs were checked field-by-field against the kernel's real `fb_var_screeninfo`/`fb_fix_screeninfo` layouts and match. |
| `vakt-compositor/src/main.rs:104-105` | `MaybeUninit::assume_init` after the `ioctl`s | Sound — only reached after both calls report success, which is the kernel's contract for having filled the buffers. |
| `vakt-compositor/src/main.rs:109` | `MmapMut::map_mut` | Sound — the framebuffer's lifetime outlives the mapping, and this process is Landlock-confined to that one device. See the compositor underflow fix elsewhere in this pass for the one real bug found near this code (not in the `unsafe` itself — the accent-square sizing above it). |
| `vakt-net/src/notify.rs:74` | `env::remove_var` in a test | Sound — the only test in the file touching this variable; no cross-test race. |

Nothing here needed changing beyond the test-race fix above.

## Fuzzing

New: `pkg-manager/fuzz/`, three `cargo-fuzz` targets covering the code that
handles bytes an attacker controls before anything trusts them:

- **`safe_relative`** — the path-safety check every archive entry goes
  through before it can be written or later removed. Doesn't just check for
  panics: the harness re-validates that anything the function accepts is
  actually safe to join onto an install root, so a fuzz-found bypass would
  fail loudly as a real traversal bug, not a missed crash.
- **`manifest_parse`** — `PackageManifest`'s JSON deserialization, exercised
  directly on the bytes a repository's `.json` response would contain.
- **`verify_signature`** — the Ed25519/SHA-256 check, fuzzed across
  structured `(data, signature_hex, public_key_hex)` inputs via `arbitrary`,
  since all three of those come from the same untrusted download.

`pkg-manager` gained a `src/lib.rs` so the fuzz crate has something to link
against — a from-scratch binary crate has no library target otherwise. It
changes nothing about the shipped binary; `main.rs` still declares its own
module tree over the same files, which is the ordinary way a package has
both a lib and a bin target.

Requires nightly Rust (`rustup toolchain install nightly`) and
`cargo install cargo-fuzz`; not part of default CI, since libFuzzer-based
fuzzing needs sanitizer instrumentation CI's stable toolchain doesn't build
with, and a real campaign runs far longer than a CI job should block on.

```bash
cd pkg-manager
cargo +nightly fuzz run safe_relative      # or manifest_parse, verify_signature
```

Each target ran clean for a short smoke-test during this pass (millions of
executions, no crashes) — enough to confirm the harnesses actually exercise
the code, not that the code has been exhaustively fuzzed. Worth running each
for longer (hours, not seconds) periodically, and especially after touching
any of the three functions above.

## Finding: release builds wrapped integer overflow silently

The Rust side had the same shape of problem as the Zig one below. No crate
configured `[profile.release] overflow-checks`, and Rust's default for release
is to **wrap silently** - confirmed, not assumed: the same underflow panics in
debug and prints `4294967292` in release. Every binary in the image is built
`cargo build --release`.

This is not hypothetical here either. The compositor underflow noted further
down this document was exactly that: arithmetic on `ioctl`-derived screen
geometry producing a wrong value rather than failing. `overflow-checks` turns
that class of bug into a loud stop instead of a bad number.

**Fixed** by setting `overflow-checks = true` in the release profile of all
five crates. Verified afterwards by running every test suite *in release*
(they normally run in debug, where the checks were already on, so release is
the only run that exercises the change): nothing in the existing code
overflows.

**The trade-off, stated plainly:** in `vakt-init` this adds implicit panic
paths to arithmetic, and `vakt-init` is PID 1 - a panic there is a kernel
panic. That crate was deliberately written with zero panic sources in
production code (verified: no `unwrap`, `expect`, `panic!` or `unreachable!`
outside its tests), so this does cut against that property. It is enabled
there anyway for two reasons: its real overflow surface is negligible
(restart counters on `u32`, byte totals on `u64`, and the one subtraction in
`update.rs` is guarded by the `Some(0)` arm above it), and on a security
appliance a loud halt is a better failure than a silently wrong value in the
process that supervises everything else. Worth revisiting if a future change
gives PID 1 real arithmetic on attacker-influenced numbers.

## Finding: the independent verifier shipped without safety checks

`vakt-verify` was built with `zig build -Doptimize=ReleaseSmall`. In Zig,
`ReleaseSmall` and `ReleaseFast` **disable runtime safety checks**; only
`Debug` and `ReleaseSafe` keep them. Confirmed rather than assumed - the same
out-of-bounds read panics under `ReleaseSafe` and silently returns adjacent
memory under `ReleaseSmall`, exiting 0.

That is the wrong trade for this binary specifically. Its entire job is to be
a *trustworthy second opinion* on attacker-supplied bytes: manifest JSON, hex
strings, and whole archives fetched off the network. A verifier whose own
bounds checks are compiled out is a weaker second opinion than its presence
implies, and it runs rarely enough (build-time re-verification, occasional
manual use) that speed and size barely matter.

**Fixed** by building `ReleaseSafe` and stripping debug info in `build.zig`.
Stripping is what makes this nearly free: unstripped `ReleaseSafe` is 4.2MB
against `ReleaseSmall`'s 229KB, but stripped it is 460KB - about 231KB for
restored bounds and overflow checking. The panic message a safety check
produces is a runtime string and still prints; only the stack trace loses its
symbols. Verified afterwards end to end against real `zrpkg`-signed packages:
a valid signature passes, a tampered archive fails, and a malformed manifest
is a clean error rather than a crash.

No fuzzing equivalent exists yet for `vakt-verify` (Zig) — Zig's own fuzzing support
is newer and less established than `cargo-fuzz`'s libFuzzer integration, and
setting it up properly is worth its own pass rather than a rushed addition
here. `vakt-verify`'s hex/JSON parsing is the other place untrusted,
network-sourced bytes get parsed before a trust decision is made from them,
and belongs on the list for whoever picks this up next.
