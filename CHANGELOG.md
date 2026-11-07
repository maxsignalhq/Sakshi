# Changelog

All notable changes to Sakshi. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## Unreleased

### Added
- `sakshi --worktree <agent>`: run the first session in its own git worktree on a new
  `sakshi/<id>` branch. Opt-in; Sakshi prints the merge and remove commands on exit and
  does neither itself.
- `sakshi report --format pr`: a short Markdown block for a pull request description
  (verdict, findings, tests, files edited) with no local ids or absolute paths.
- `session-overlap` trust rule (warn): two running sessions edited the same file, so
  their changes may collide.
- `outside-project` trust rule (warn): the agent edited a file outside the directory
  the session started in.
- `manifest-changed` trust rule (info): a dependency manifest such as `Cargo.toml` or
  `package.json` changed, so you can check for packages the agent added.
- `repeated-failure` trust rule: warns when the same test command has failed three
  times in a row, a sign the agent is stuck.

### Fixed
- Codex now reports to Sakshi even when `sakshi` is not on your `PATH` (a build
  run from its folder, a custom install location). Sakshi puts its own folder on
  the `PATH` it gives Codex.
- A Codex too old to accept `--no-daemon` still starts without live state, and
  `sakshi.log` now says why instead of the session just turning yellow.

## [0.2.0] - 2026-11-07

### Added
- Codex support: live state (working, needs you, done), notifications and the
  command, file-edit and test trust rules, for Codex running inside Sakshi.
  Start it with `o` or `sakshi codex`. Codex only runs hooks you have approved,
  so it needs a one-time `sakshi setup codex` (asks first; `--status` to check,
  `--remove` to undo) and an approval in Codex. Not available on Windows yet.
  Sakshi starts Codex with `--no-daemon` (when your Codex supports it), so each
  session has its own app server and its hooks reach the right session; they
  would otherwise run in Codex's shared background server and report nothing.
- Install with Homebrew (`brew install maxsignalhq/tap/sakshi`) or Scoop (`maxsignalhq/scoop-bucket`).

### Changed
- The `hooks-silent` finding now points Codex users at `sakshi setup codex`.

### Fixed
- Trust evaluation no longer panics on a file path containing non-ASCII
  characters that lies outside the session directory (#17).
- The generated Homebrew formula now passes `brew audit --strict --new`.

## [0.1.0] - 2026-10-24

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
- One-line installers for macOS/Linux (`install.sh`) and Windows
  (`install.ps1`) that verify the release checksum, and Windows ARM64 builds.
- Contributor docs: CONTRIBUTING, Code of Conduct, security policy, issue and
  pull request templates, and a guide to writing adapters for other agents.
- Change tracking against a snapshot taken when a session starts, so only the
  session's own changes count.
- A "while you were away" summary when you re-attach, and a details panel (`t`).
- Per-session activity logs, and `sakshi sessions`, `sakshi report` (Markdown or
  JSON) and `sakshi replay` to look back at them.
- A local line protocol (PROTOCOL.md) over a Unix socket or Windows named pipe;
  hooks always fail open.
- macOS, Linux and Windows support, with CI on all three.
- Release workflow and Homebrew, Scoop and winget manifest templates.

### Fixed before release
- Security: the per-process runtime directory is now created atomically under
  an unpredictable name and refuses to adopt an existing path. Before, on a
  shared Linux machine with no `XDG_RUNTIME_DIR`, another user could plant
  `/tmp/sakshi-<pid>` first and swap the hook settings or the socket.
- Activity logs and `sakshi.log` are created readable by the user only on Unix,
  and obvious secrets in commands are masked before they reach the disk.
- Pasting several lines no longer submits each line as it arrives: terminals
  that report pastes get the text marked as a paste (bracketed paste), and on
  Windows a burst of typed characters with a line break is treated as one.
- Ctrl, Shift and Alt with arrows, Home, End, PageUp/PageDown, Insert, Delete
  and function keys now reach the program (Ctrl+Left jumps a word).
- Scrollback: Shift+PgUp/PgDn while attached, PgUp/PgDn on the list.
- Trust: commands are split like a shell would, so quoted text is no longer
  mistaken for a command, and `bash -c`, `cmd /c`, `powershell -Command`,
  `sudo`, `env`, `nohup` and `xargs` no longer hide a destructive command.
- Killing a session kills the programs it started too.
- Tested against a real Claude Code: its hooks drive the session state and the
  commands it runs are recorded (`cargo test -p sakshi --test claude_real --
  --ignored`).

### Known limits
- Sessions end when Sakshi exits; there is no background daemon.
- Live state exists for Claude Code only.
- Agents inside WSL cannot report to a Windows-hosted Sakshi; run Sakshi inside WSL.
