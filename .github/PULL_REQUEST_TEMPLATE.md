## What and why

<!-- What does this change, and why is it needed? Link the issue if there is one. -->

## How I checked it

<!-- Tests added, what you ran by hand, and on which OS / terminal / agent. -->

## Checklist

- [ ] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` pass
- [ ] Tests cover the change (for trust rules: what it catches and what it must not flag)
- [ ] TRUST.md / PROTOCOL.md / README updated if behaviour changed
- [ ] `CHANGELOG.md` has an entry under `Unreleased`
- [ ] Nothing new leaves the user's machine
