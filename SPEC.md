# Sakshi — Spec

Sakshi (साक्षी, "the witness") is an open-source, cross-platform cockpit for running and supervising AI coding agents. Anyone using Claude Code (first), Codex, Gemini CLI, or similar agents can see what each agent is doing, get notified when one needs them, and know at a glance whether its work can be trusted.

## Principles
- Useful in 5 minutes: install, run, see value. Zero config by default.
- Calm by default: quiet when fine, clear signals when not.
- Runs inside any terminal on macOS, Windows, and Linux. No tmux dependency.
- Local-first: no accounts, no telemetry, nothing leaves the machine.
- Trust is the edge: every session shows what changed and whether anything looks off.

## Stack
- Rust stable, Cargo workspace, single binary per platform, MIT license.
- TUI: ratatui + crossterm. Async: tokio.
- PTY: portable-pty (Unix PTY + Windows ConPTY). Terminal emulation: vt100 crate (swappable behind a trait).
- IPC: Unix domain socket (macOS/Linux) and named pipe (Windows) behind an `IpcTransport` trait.
- Paths: `directories` crate. Errors: thiserror (libs) + anyhow (bin). No unwrap/expect outside tests.
- Windows shells: PowerShell, cmd, WSL.

## Workspace layout
```
crates/
  core/      session model, event bus, trust engine (no UI, no OS code)
  pty/       PTY spawn, resize, I/O
  ipc/       transports + protocol types
  adapters/  agent adapters (claude first)
  tui/       ratatui app
  cli/       binary entry point + subcommands (binary name: sakshi)
```

## Concepts
- **Session:** id, agent kind, working dir, state (idle | working | waiting | done | error), started_at.
- **Event:** typed, timestamped (state, tool_use, file_edit, command, test_run, message).
- **Adapter:** trait per agent; capability-probed at runtime; unavailable agents hidden.
- **Trust engine:** pure functions over events + git diff → findings with severity and reason → badge (green | yellow | red).
- **Activity log:** append-only JSONL per session in the app data dir.

## IPC protocol (v1)
Newline-delimited JSON:
```json
{"v":1,"session_id":"...","type":"state|tool_use|file_edit|command|test_run|message","ts":"RFC3339","data":{}}
```
Malformed lines are logged and skipped; the connection stays open. Documented in PROTOCOL.md.

## UI layout
Left: session list (state icon, name, trust badge). Center: attached terminal. Right (toggle): summary, trust findings, recent events. Footer: key hints.

## Standards
- CI on macOS, Windows, and Linux from phase 1 (GitHub Actions: fmt, clippy -D warnings, test).
- Unit tests for protocol parsing, event bus, and every trust rule.
- Small modules, one responsibility. Public items documented.

## Roadmap
1. Foundation: workspace, PTY, one live terminal session in the TUI.
2. Agent awareness: IPC server + Claude Code hooks adapter, live session state.
3. Multiple sessions, attach/detach, desktop notifications.
4. Trust engine: findings, badges, activity log.
5. "While you were away" summary, reports, replay.
6. Release: binaries, Homebrew/Scoop/winget, docs.

## Working agreement
Build in phases. Each phase: complete files with paths, build/run commands, and verification steps per OS. Stop at the end of the phase. Flag anything in this spec that is wrong, risky, or overbuilt before coding.
