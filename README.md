# Sakshi

**The witness for your AI coding agents.**

Sakshi (साक्षी, Sanskrit for "the witness") runs and supervises AI coding agents like Claude Code, Codex, and Gemini CLI from one terminal. See what every agent is doing, get notified when one needs you, and know at a glance whether its work can be trusted.

> Status: early development. Not ready for use yet.

## Why
Running agents is easy. Knowing what they did is not. Sakshi watches each session and tells you when something looks off: tests skipped or deleted, assertions weakened, sensitive files touched, destructive commands run.

## Planned features
- Run several agent sessions side by side, inside any terminal
- Live status for each session: working, waiting for you, done, error
- Desktop notifications when an agent needs attention
- Trust badges with plain-English reasons
- "While you were away" summaries and session replay
- macOS, Windows, and Linux. Local-first: no accounts, no telemetry

## Roadmap
See [SPEC.md](SPEC.md).

## License
[MIT](LICENSE)
