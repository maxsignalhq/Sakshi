# Changelog

All notable changes to Sakshi. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## Unreleased

First usable version.

### Added
- A terminal UI that runs any program, such as your shell or an AI coding agent,
  in a session list beside its live terminal. Attach and detach (Ctrl+]),
  several sessions at once, new sessions with `n`.
- Claude Code support: hooks are injected with `--settings` (your own settings
  are not touched) to give live state: working, needs you, done.
- Desktop notifications when a session you are not looking at needs you,
  finishes, fails or turns red (`--no-notify` to opt out).
- A trust engine with a green/yellow/red badge per session and plain-English
  reasons: deleted or skipped tests, weakened assertions, secrets and CI files
  touched, destructive commands, failing or missing test runs. See TRUST.md.
- Trust rules for business-critical code (authentication, payments, database
  migrations), lowered coverage thresholds, added lint/type/coverage
  suppressions, changed dependency lockfiles, and Claude Code hooks that have
  gone silent.
- An optional `config.toml` to mark your own sensitive paths or ignore some
  (`SAKSHI_CONFIG` overrides its location).
- Change tracking against a snapshot taken when a session starts, so only the
  session's own changes count.
- A "while you were away" summary when you re-attach, and a details panel (`t`).
- Per-session activity logs, and `sakshi sessions`, `sakshi report` (Markdown or
  JSON) and `sakshi replay` to look back at them.
- A local line protocol (PROTOCOL.md) over a Unix socket or Windows named pipe;
  hooks always fail open.
- macOS, Linux and Windows support, with CI on all three.
- Release workflow and Homebrew, Scoop and winget manifest templates.

### Known limits
- Sessions end when Sakshi exits; there is no background daemon.
- Live state exists for Claude Code only.
