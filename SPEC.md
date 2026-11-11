# Sakshi — Spec

Sakshi (साक्षी, "the witness") answers one question about an AI coding agent:
**can I trust what it just did?**

You run the agent inside Sakshi exactly as you would in any terminal. Sakshi
watches what it does, cross-checks what the agent reports against what actually
changed on disk, and gives each session a verdict (green, yellow or red) with
plain-English reasons. Everything else in Sakshi exists to make that verdict
available at the moment you need it.

## What Sakshi is

- **A witness.** It observes and reports. It never edits, merges, reverts,
  blocks or approves anything on your behalf.
- **A verdict with reasons.** Every finding answers one of five questions:
  1. *Faked success*: did the agent make a check pass without doing the work?
  2. *Risky actions*: did it do something dangerous?
  3. *Not verified*: has the work been tested since it last changed?
  4. *Worth reviewing*: what deserves a human look before you accept it?
  5. *Blind spots*: what could Sakshi not see?
- **A terminal you already know.** One binary, runs inside any terminal on
  macOS, Windows and Linux. Several sessions side by side, because that is how
  people run agents, and so that Sakshi can tell you which one needs you.
- **Local.** No accounts, no telemetry, no network calls.

## What Sakshi is not

These are decisions, not gaps. A change that needs one of them is out of scope.

- **Not a framework or an orchestrator.** It does not start agents on a
  schedule, hand work between them, or let them message each other.
- **Not an actor.** It does not fix, merge, roll back or gate. The most it does
  is tell you, and print the commands you might run.
- **Not a sandbox.** It does not restrict what an agent can do.
- **Not a judge of correctness.** Rules are heuristics over commands, paths and
  diffs. A badge is a prompt to look, never proof.
- **Not a service.** No daemon, no server, no team dashboard, no plugins.
- **Not a catalogue of agents.** Claude Code and Codex are supported and tested
  against the real thing. Another agent is added when someone asks for it and it
  can be tested for real. Anything can report without an adapter by writing a
  line of JSON to the socket (PROTOCOL.md).

## Principles

- **The verdict comes to you.** In the session list, in the details panel, in a
  notification when a session turns red, and as the last thing printed when you
  quit. You should never have to go looking for it.
- **Calm by default.** Quiet when fine, clear when not. A finding that cries
  wolf is a bug.
- **Honest about its limits.** When Sakshi cannot see, it says so
  (`hooks-silent`) instead of showing green.
- **Fail open.** If Sakshi is not running or breaks, the agent is unaffected.
- **Hands off your setup.** Sakshi never edits an agent's own config. The one
  exception is the explicit, reversible `sakshi setup <agent>`.
- **Useful in five minutes.** Install, run, see value. Zero config by default.

## How it works

Sakshi learns what an agent does from two independent sources and the trust
engine cross-checks them:

- **What the agent reports.** The agent's hooks run `sakshi hook <agent>`, which
  sends events (commands, file edits, test runs, state) to Sakshi over a local
  socket or named pipe.
- **What actually changed.** A snapshot of the git repository at session start,
  and diffs against it.

The trust engine is a pure function: events + diff + observations → findings →
badge. Rules are listed in TRUST.md.

```
crates/
  core/      session and event model, event bus, trust engine, reports (no UI, no OS code)
  pty/       PTY spawn, resize, I/O
  ipc/       transports + protocol types
  adapters/  agent adapters (Claude Code, Codex)
  git/       repository baseline snapshots and diffs
  store/     activity logs, data and config locations
  tui/       ratatui app
  cli/       binary entry point, subcommands, config loading (binary name: sakshi)
```

Stack: Rust stable, one binary per platform, MIT. ratatui + crossterm, tokio,
portable-pty, vt100. Unix domain socket or Windows named pipe behind a transport
trait. thiserror in libraries, anyhow in the binary, no `unwrap`/`expect`
outside tests.

## What 1.0 keeps stable

From 1.0.0, within the 1.x series:

| Surface | Promise |
|---------|---------|
| Wire protocol `v: 1` (PROTOCOL.md) | Message shape and existing event types do not change. New event types and new optional fields may be added; readers must ignore what they do not know. A breaking change is `v: 2`. |
| Activity logs | Logs written by any 1.x can be read by any later 1.x. |
| Rule names and concern names (TRUST.md) | Never renamed or reused. New rules may be added. |
| `sakshi report --format json` | Existing fields keep their names and meaning. Fields may be added. |
| `config.toml` | Existing keys keep working. |
| Command line | Existing subcommands and flags keep working. `Ctrl+]` stays the detach key. |
| Codex plugin files | Frozen: a change would re-prompt every user for approval. |

Deliberately **not** promised, because they have to stay free to improve:

- What a rule matches and its severity. Rules are heuristics and are tuned as
  false positives and misses are found; every such change is in CHANGELOG.md.
- The wording of findings, Markdown reports and the exit summary. Scripts
  should read the JSON report.
- The layout of the TUI and the keys on the session list.
- The Rust crates. They are internal to the binary and not published.

## Standards

- CI on macOS, Windows and Linux: fmt, clippy `-D warnings`, test.
- Every trust rule has a test for what it catches and for what it must not flag.
- A behaviour change updates TRUST.md, PROTOCOL.md and CHANGELOG.md with it.
- Small modules, one responsibility. Public items documented.

## Deciding what gets in

A proposed change belongs in Sakshi if all of these hold:

1. It makes the answer to "can I trust what it just did?" more accurate, more
   honest, or easier to reach.
2. Sakshi still only watches.
3. Nothing leaves the machine.
4. It works with zero configuration.

docs/FUTURE.md lists ideas that pass, and ones that were considered and left out.
