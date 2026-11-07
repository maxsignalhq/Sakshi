# How Sakshi decides what to trust

Every session gets a badge:

| Badge | Meaning |
|-------|---------|
| green | Nothing looks off |
| yellow | Something deserves a look |
| red | Something looks seriously wrong |

The badge is the worst severity among the findings. Each finding has a rule
name, a severity (`info`, `warn` or `alert`) and a plain-English reason. `info`
findings are shown but never change the badge.

Sakshi can be wrong in both directions. The badge is a prompt to look, not a
verdict. Rules are heuristics over text: they do not run your code.

## What it looks at

- **Events** the agent reports (see PROTOCOL.md): commands, file edits, test runs.
- **The diff** of the session's working tree. When a session starts, Sakshi
  records the state of the git repository it is in (with `git stash create`,
  which stores a snapshot commit without touching your files). Later diffs show
  only what changed since then, so work you had already is not blamed on the
  agent. New untracked files count; git-ignored files do not appear in the diff
  (a git-ignored `.env` is still caught if the agent reports editing it).

Without events (a plain shell) or outside a git repository, the rules that need
them simply do not fire.

## Rules

| Rule | Severity | Fires when |
|------|----------|------------|
| `tests-deleted` | alert | A test file was deleted |
| `tests-skipped` | warn | Lines were added that switch tests off or narrow a run: `#[ignore]`, `@pytest.mark.skip`, `.skip(`, `.only(`, `xit(`, `t.Skip(`, `@Disabled`, `@Ignore`, ... (test files, plus `#[ignore]` anywhere in Rust) |
| `assertions-weakened` | warn | Test files lost more assertion lines than they gained |
| `trivial-assertion` | warn | A test gained an assertion that is always true, such as `assert!(true)` |
| `sensitive-file` | alert | A secrets file was touched: `.env`, `*.pem`, `*.key`, `id_rsa`, `.npmrc`, `credentials*`, anything under `.ssh/`, `.aws/`, `.gnupg/`, `.kube/`, or git internals under `.git/` |
| `sensitive-file` | warn | CI or automation config was touched: `.github/workflows/`, `.gitlab-ci.yml`, `Jenkinsfile`, `.husky/`, `CODEOWNERS`, ... |
| `sensitive-file` | warn | Business-critical code was touched: a path with a word like `auth`, `login`, `oauth`, `jwt` (authentication), `payment`, `billing`, `invoice`, `ledger`, `checkout` (payments) or `migration`, `migrate`, `alembic`, `flyway` (database migrations). Whole words only, so `author.rs` is fine; test and doc files are left to the test rules |
| `sensitive-file` | yours | A path matched a pattern in your [config](#your-own-rules) |
| `coverage-lowered` | alert | A coverage threshold went down: `fail_under`, `--cov-fail-under`, `--fail-under-lines`, Jest/nyc/Vitest `coverageThreshold` numbers, codecov `target`, ... |
| `checks-suppressed` | warn | Lines were added that silence a check instead of fixing it: `eslint-disable`, `@ts-ignore`, `@ts-nocheck`, `# noqa`, `# type: ignore`, `#[allow(`, `@SuppressWarnings`, `//nolint`, `istanbul ignore`, ... (doc files are skipped) |
| `lockfile-changed` | info | A dependency lockfile changed: `Cargo.lock`, `package-lock.json`, `yarn.lock`, `pnpm-lock.yaml`, `poetry.lock`, `uv.lock`, `go.sum`, ... |
| `manifest-changed` | info | A dependency manifest changed: `Cargo.toml`, `package.json`, `pyproject.toml`, `requirements*.txt`, `go.mod`, `Gemfile`, ... Look for packages the agent added |
| `destructive-command` | alert | `git push --force`, `git reset --hard`, `git clean -f`, `rm -rf` of `/`, `~`, `*`, `.` and similar, `DROP TABLE` / `DROP DATABASE`, `dd` onto a device, `mkfs` |
| `destructive-command` | warn | Other `rm -rf`, `git push --force-with-lease`, `git checkout .`, `git restore .`, `git branch -D`, `chmod -R 777`, piping `curl`/`wget` into a shell, PowerShell `Remove-Item -Recurse -Force`, `rd /s` |
| `tests-failing` | warn | The latest test run failed |
| `repeated-failure` | warn | The same test command failed 3 or more times in a row (replaces `tests-failing`): the agent may be stuck |
| `unverified-changes` | info | Files changed and no test run came after the last edit |
| `hooks-silent` | warn | Claude Code or Codex has printed plenty for a minute or more but its hooks have reported nothing, so live state and event-based findings may be missing. For Codex this usually means `sakshi setup codex` has not been run or its hooks are not approved yet. Clears as soon as a report arrives |

Template env files (`.env.example`, `.env.sample`) and public keys (`*.pub`)
are not flagged. Paths inside the session's working directory are matched
relative to it, so a project folder named `payments-api` does not make every
file in it "payments code".

## Your own rules

Sakshi reads an optional `config.toml`. Without one, the built-in rules apply
unchanged.

```toml
[trust]
# Extra paths to flag when an agent touches them.
# severity is "info", "warn" (the default) or "alert".
sensitive_paths = [
  { pattern = "src/ledger/**", severity = "alert" },
  { pattern = "**/kyc/**" },
]
# Paths never reported as sensitive, not even by the built-in rules.
ignore_paths = ["fixtures/**", "src/auth/test_helpers.rs"]
```

Patterns are globs over `/`-separated paths, case-insensitive: `*` and `?`
match within one folder, `**` across folders. A pattern that does not start
with `/` can match at any depth, so `**/kyc/**` and `kyc/**` behave the same.
`ignore_paths` only affects `sensitive-file` findings.

The file lives at:

| OS | Location |
|----|----------|
| Windows | `%APPDATA%\maxsignalhq\sakshi\config\config.toml` |
| macOS | `~/Library/Application Support/dev.maxsignalhq.sakshi/config.toml` |
| Linux | `~/.config/sakshi/config.toml` |

or wherever `SAKSHI_CONFIG` points. A file with a typo or an unknown key stops
Sakshi with an error instead of being ignored, so a mistake never quietly turns
a rule off.

## Where the verdict shows up

- A coloured dot after each session in the list.
- The details panel (`t` in the list): verdict, findings, a change summary and
  recent activity.
- A desktop notification when a session you are not looking at turns red.
- The activity log, which records every change of verdict.

## How commands are read

Commands are split the way a shell would split them, honouring quotes, so
`git commit -m "never run git push --force; rm -rf /"` is a commit, not a
force-push and a wipe. Wrappers are looked through: `sudo`, `env`, `nohup`,
`time`, `xargs`, leading `NAME=value` assignments, and the scripts handed to
`bash -c` (and `sh`, `zsh`, ...), `cmd /c` and `powershell -Command`, up to three
levels deep. It is still a pattern match: a destructive command hidden in a
script file, built up from variables, or run through a program Sakshi does not
know will not be seen.

## Activity log

Each session appends to `<data dir>/sessions/<start time>-<session id>.jsonl`,
one protocol message per line. The data directory is
`%LOCALAPPDATA%\maxsignalhq\sakshi\data` on Windows,
`~/Library/Application Support/dev.maxsignalhq.sakshi` on macOS and
`~/.local/share/sakshi` on Linux, or whatever `SAKSHI_DATA_DIR` says. Nothing is
ever sent anywhere. `--no-log` turns logging off.

Logs hold the commands an agent ran and the paths it touched, so on Unix the
directory and files are readable by your user only. Before a command is written
to disk, obvious secrets are masked as `[redacted]`: bearer tokens, the value
after `--token`, `--password`, `--secret` and similar flags, `NAME=value` where
the name contains `token`, `secret`, `password` or `key`-like words, well-known
key prefixes (GitHub, GitLab, Anthropic, Slack, npm, AWS, Google), and the
password in `https://user:password@host`. This is best effort: a secret in an
unusual shape, or a quoted value with spaces in it, can slip through. The live
details panel shows commands as they happened; only what reaches the disk, and
so reports and replays, is masked.
