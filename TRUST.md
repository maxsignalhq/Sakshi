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
| `destructive-command` | alert | `git push --force`, `git reset --hard`, `git clean -f`, `rm -rf` of `/`, `~`, `*`, `.` and similar, `DROP TABLE` / `DROP DATABASE`, `dd` onto a device, `mkfs` |
| `destructive-command` | warn | Other `rm -rf`, `git push --force-with-lease`, `git checkout .`, `git restore .`, `git branch -D`, `chmod -R 777`, piping `curl`/`wget` into a shell, PowerShell `Remove-Item -Recurse -Force`, `rd /s` |
| `tests-failing` | warn | The latest test run failed |
| `unverified-changes` | info | Files changed and no test run came after the last edit |

Template env files (`.env.example`, `.env.sample`) and public keys (`*.pub`)
are not flagged.

## Where the verdict shows up

- A coloured dot after each session in the list.
- The details panel (`t` in the list): verdict, findings, a change summary and
  recent activity.
- A desktop notification when a session you are not looking at turns red.
- The activity log, which records every change of verdict.

## Activity log

Each session appends to `<data dir>/sessions/<start time>-<session id>.jsonl`,
one protocol message per line. The data directory is
`%LOCALAPPDATA%\maxsignalhq\sakshi\data` on Windows,
`~/Library/Application Support/dev.maxsignalhq.sakshi` on macOS and
`~/.local/share/sakshi` on Linux, or whatever `SAKSHI_DATA_DIR` says. Nothing is
ever sent anywhere. `--no-log` turns logging off.
