# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

Read SPEC.md before any work. It is the source of truth for scope, stack, architecture, and the phased roadmap.

## Rules
- Work one roadmap phase at a time. Stop at the end of the phase and report how to verify it.
- Keep `crates/core` free of UI and OS-specific code.
- No `unwrap`/`expect` outside tests. Use thiserror in libraries, anyhow in the binary.
- Every change must pass: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`.
- Code must build and behave on macOS, Windows, and Linux. Gate platform code with `cfg` and keep it behind traits.
- Hooks and adapters fail open: if Sakshi isn't running, agents must be unaffected.
- Never touch an agent's own config files. The only exception is `sakshi setup <agent>`: explicit, asks first, goes through the agent's own CLI, and is reversible with `--remove`.
- Nothing leaves the machine: no telemetry, no network calls.
- If the spec looks wrong, risky, or overbuilt, say so before writing code.

## Commands
```sh
cargo run -p sakshi -- claude        # run the binary (package `sakshi`; other crates are `sakshi-<name>`)
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
cargo test -p sakshi-core trust::    # one crate / one module; append a test name to narrow further
python3 -m unittest discover -s packaging   # package-manifest + installer tests (also run in CI)
```
- `unwrap_used`/`expect_used` are `deny` and `unsafe_code` is `forbid` workspace-wide. Test code opts out per crate with `#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]`; integration tests use `#![allow(...)]`.
- `crates/cli/tests/` are end-to-end: they run the real `sakshi` binary inside a PTY and read its screen through a terminal emulator (`tests/common/mod.rs`). `SAKSHI_TEST_BIN` points them at a different (e.g. released) binary.
- `tests/claude_real.rs` talks to a real logged-in `claude` and is `#[ignore]`d: `cargo test -p sakshi --test claude_real -- --ignored --nocapture`.
- `tests/codex_real.rs` does the same for a real logged-in `codex` (Unix only, uses a throwaway `CODEX_HOME`, starts a shared Codex server first): `cargo test -p sakshi --test codex_real -- --ignored --nocapture`.
- Env vars: `SAKSHI_DATA_DIR` (logs location), `SAKSHI_CONFIG`, `SAKSHI_LOG=debug`. `SAKSHI_ENDPOINT` / `SAKSHI_SESSION_ID` are set by Sakshi for child processes so hooks can call back.

## Status
The roadmap in SPEC.md is complete and 0.4.0 is released; the "one phase at a time" rule applies to new work. Behaviour changes must also update TRUST.md (rules), PROTOCOL.md (wire protocol) and the `Unreleased` section of CHANGELOG.md. Every trust rule needs a test for what it catches *and* what it must not flag.

## Architecture
Sakshi runs agents inside PTYs it owns, and learns what they do from two independent sources that the trust engine then cross-checks.

**Data flow.** `pty` spawns the program and feeds a `vt100` emulator (`tui/emulator.rs`) for the screen. Agents report events a second way: the adapter's `launch_args` inject a generated config (for Claude: `--settings <file>`, never touching the user's own settings) whose hooks run `sakshi hook <agent>`. That shim (`cli/src/hook.rs`) reads the payload from stdin, finds Sakshi via `SAKSHI_ENDPOINT`/`SAKSHI_SESSION_ID`, and sends newline-delimited JSON over a Unix socket or Windows named pipe (`ipc`, behind a transport trait). It must never print, block (500 ms timeout) or exit non-zero. The `ipc` server feeds events into the `core` `EventBus`; the TUI workspace, the activity-log writer (`store`) and notifications are all subscribers.

**Crates.**
- `core`: session/event model, `EventBus`, trust engine (`trust.rs`, by far the largest file), diff parsing, summaries, reports, secret redaction. Trust evaluation is pure: `evaluate_with(events, diff, config, observations)` returns findings → green/yellow/red badge. `Observations` carry non-event facts (e.g. terminal output volume) used by rules like `hooks-silent`.
- `adapters`: `AgentAdapter` trait (match program, probe install, `launch_args`, `translate_hook` payload → `Event`s). `ClaudeAdapter` injects hooks per session; `CodexAdapter` relies on a one-time `sakshi setup codex` plugin whose files are frozen (any change re-prompts every user for approval) and launches Codex with `--no-daemon`, because Codex runs hooks in a shared background server that never sees a session's `SAKSHI_*` env; `docs/ADAPTERS.md` is the guide for adding one, which also needs an `AgentKind` variant in `core/session.rs`.
- `git`: baseline snapshot at session start and diffs against it (this is how "what changed" is blamed on a session). `store`: per-session append-only JSONL logs, data/config dirs, owner-only permissions. Events are redacted (`core::redact`) before being written.
- `tui`: `workspace.rs` is pure state (keys and events in, "what the caller must do" out; no I/O), `app.rs` is the event loop that does the I/O, `ui.rs` renders. Keep that split.
- `cli`: clap entry point, config loading, the hidden `hook` subcommand, and the read-only `sessions` / `report` / `replay` commands that work from the stored logs.

**Wire protocol** (PROTOCOL.md): `{"v":1,"session_id":...,"type":...,"ts":...,"data":{}}`, one per line. Malformed lines are logged and skipped, never fatal. Anything that can write a line of JSON to the socket can report without an adapter.

## Reference docs
README.md (user-facing behaviour and known limits), TRUST.md (every rule), PROTOCOL.md, CONTRIBUTING.md, docs/RELEASING.md (releases and packaging are manual), `scripts/demo` (reproducible demo GIF with a scripted fake agent).
