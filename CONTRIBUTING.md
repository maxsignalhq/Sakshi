# Contributing to Sakshi

Thanks for helping. Sakshi is young, so bug reports, trust-rule feedback and
adapters for new agents are all especially valuable.

## Good first contributions

- **Trust-rule feedback.** A finding that was wrong, or something an agent did
  that Sakshi missed. Use the "Trust rule" issue template; a small diff or command
  that reproduces it is gold.
- **Adapters** for other agents (Codex, Gemini CLI, Aider, OpenCode, ...). Start
  with [docs/ADAPTERS.md](docs/ADAPTERS.md).
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

- Read [SPEC.md](SPEC.md) and [CLAUDE.md](CLAUDE.md); they hold the design and the
  working rules (which apply to people too).
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
