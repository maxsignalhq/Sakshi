# Contributing to Sakshi

Thanks for helping. Sakshi does one thing: it tells you whether an AI coding
agent's work can be trusted. The most valuable contributions make that answer
more accurate.

## Good first contributions

- **Trust-rule feedback.** A finding that was wrong, or something an agent did
  that Sakshi missed. Use the "Trust rule" issue template; a small diff or command
  that reproduces it is gold.
- **Requests for another agent.** Claude Code and Codex are supported. Open an
  issue naming the agent you use; an adapter is added when it can be tested
  against the real agent. [docs/ADAPTERS.md](docs/ADAPTERS.md) is the guide.
- **Platform reports.** If something renders or behaves differently on your OS,
  terminal or shell, say so, with versions.

For anything larger than a small fix, open an issue first so we can agree on the
approach before you spend time on it.

## Building

You need a recent stable [Rust](https://rustup.rs) and `git`.

```sh
git clone https://github.com/maxsignalhq/Sakshi
cd Sakshi
cargo run -p sakshi            # your shell inside Sakshi
cargo run -p sakshi -- claude  # Claude Code inside Sakshi
```

## Before you open a pull request

All of these must pass; CI runs them on macOS, Linux and Windows:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
python3 -m unittest discover -s packaging
```

And please:

- Read [SPEC.md](SPEC.md) and [CLAUDE.md](CLAUDE.md); they hold the direction
  (including what Sakshi deliberately is not, and what 1.0 keeps stable) and the
  working rules (which apply to people too).
- A new trust rule belongs to one of the five concerns in [TRUST.md](TRUST.md):
  add it to `Concern::of_known_rule` and to the matching table. A test fails if
  the two disagree.
- Keep `crates/core` free of UI and OS-specific code.
- No `unwrap` or `expect` outside tests.
- Add tests with every behaviour change. Every trust rule needs a test for what
  it catches **and** for what it must not flag.
- Update [TRUST.md](TRUST.md) when rules change, [PROTOCOL.md](PROTOCOL.md) when the
  protocol changes, and the `Unreleased` section of [CHANGELOG.md](CHANGELOG.md).
- Nothing may leave the user's machine: no telemetry, no network calls.

## Pull requests

Keep them focused: one change per pull request is easier to review and revert.
Describe what changed, why, and how you checked it. Screenshots or a short
recording help for anything visible.

By contributing you agree that your work is licensed under the
[MIT License](LICENSE), and to follow the [Code of Conduct](CODE_OF_CONDUCT.md).
