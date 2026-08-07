# Security Policy

Vakt OS is a security appliance; a vulnerability here is worse than in most
projects, so please don't file it as a public issue.

## Reporting a vulnerability

Use GitHub's private reporting: this repository's **Security** tab →
**Report a vulnerability**. That opens a draft security advisory only you
and the maintainer can see, which is the right place for anything involving
the privilege-drop path, Landlock rulesets, package signature verification
(`zrpkg` or `vakt-verify`), the read-only root, or the panel's PIN gate.

Include what you'd expect: what the bug is, how to reproduce it, and what it
lets an attacker do. A minimal reproduction (a crafted package, a config
file, a command sequence) is worth more than a description of the theory.

There's no dedicated security team behind this — it's maintained by one
person — so treat response times as best-effort, not an SLA. You'll get
acknowledgement and, if the report is valid, a fix and a coordinated
disclosure timeline worked out with you.

## Scope

**In scope:** the code in this repository — `vakt-init`, `pkg-manager`
(`zrpkg`), `vakt-net`, `vakt-compositor`, `vakt-verify`, the Go tools under
`tools/cmd`, and the build/deploy scripts.

**Out of scope:** vulnerabilities in third-party components this project
uses but doesn't write — the Linux kernel, busybox, GRUB, wpa_supplicant,
and the Rust/Go/Zig dependencies listed in the README's Third-party
components section. Report those upstream. An exception: if you found the
issue *because* of how this project configures or invokes one of those
components (for example, a kernel option that should be on and isn't), that
part is in scope here even if the underlying CVE is not.

## Supported versions

Only `main` is supported. There are no maintained release branches; tagged
releases are ISO snapshots, not something that gets backported fixes.

## What the system already defends against

Read-only root, mandatory package signatures with an independent second
verifier, Landlock-sandboxed daemons, an unprivileged panel behind a PIN,
and boot-time kernel hardening — see the README's Security model section for
specifics. If your report is about one of these being weaker than
documented, that's exactly what this policy is for.
