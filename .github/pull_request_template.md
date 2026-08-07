## What this does and why

<!-- The "why" matters more than the "what" here — see CONTRIBUTING.md's
     note on comments: the same standard applies to PR descriptions. -->

## Checklist

- [ ] `cargo fmt --check` passes for every Rust crate this touches
- [ ] `gofmt -l tools` is clean, `go vet ./...` passes (if Go code changed)
- [ ] `zig fmt --check` passes (if `vakt-verify` changed)
- [ ] `bash -n` passes on any shell script changed
- [ ] Relevant tests pass locally, and new behavior has a test backing it —
      not tests that only restate the implementation (see CONTRIBUTING.md)
- [ ] Comments explain the decision, not the mechanism, where one was worth
      recording

## Security model

Does this touch the read-only root, privilege drop, Landlock rulesets,
package signature verification, the panel's PIN gate, or anything else in
the README's Security model section?

- [ ] No
- [ ] Yes — explained below, including why it doesn't weaken what's there

<!-- If yes: CONTRIBUTING.md asks for this to be stated plainly, with a
     reason better than convenience. -->
