# CLAUDE.md

Read SPEC.md before any work. It is the source of truth for scope, stack, architecture, and the phased roadmap.

## Rules
- Work one roadmap phase at a time. Stop at the end of the phase and report how to verify it.
- Keep `crates/core` free of UI and OS-specific code.
- No `unwrap`/`expect` outside tests. Use thiserror in libraries, anyhow in the binary.
- Every change must pass: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`.
- Code must build and behave on macOS, Windows, and Linux. Gate platform code with `cfg` and keep it behind traits.
- Hooks and adapters fail open: if Sakshi isn't running, agents must be unaffected.
- Nothing leaves the machine: no telemetry, no network calls.
- If the spec looks wrong, risky, or overbuilt, say so before writing code.
