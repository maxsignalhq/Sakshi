# Sakshi

**The witness for your AI coding agents.**

Sakshi (साक्षी, Sanskrit for "the witness") runs and supervises AI coding agents like Claude Code from one terminal. See what every agent is doing, get notified when one needs you, and know at a glance whether its work can be trusted.

```
┌ Sessions ──────────┐┌ claude - ~/work/app ─────────────────────────────┐
│1 ● claude ●        ││ ...the agent's own terminal, live...             │
│2 ! shell   ●       ││                                                  │
└────────────────────┘└──────────────────────────────────────────────────┘
 up/down select  Enter attach  n shell  c Claude Code  t details  q quit
```

> Status: 0.1, early. It works and is tested on macOS, Linux and Windows, but it is young, and the trust rules are heuristics (see [TRUST.md](TRUST.md)).

## Why
Running agents is easy. Knowing what they did is not. Sakshi watches each session and tells you when something looks off: tests skipped or deleted, assertions weakened, sensitive files touched, destructive commands run.

## Install

**macOS and Linux:**

```sh
curl -fsSL https://raw.githubusercontent.com/maxsignalhq/Sakshi/main/install.sh | sh
```

**Windows (PowerShell):**

```powershell
irm https://raw.githubusercontent.com/maxsignalhq/Sakshi/main/install.ps1 | iex
```

Both scripts download the build for your machine from the latest GitHub release, check its SHA-256 against the published checksum, and install `sakshi` to `~/.local/bin` (Windows: `%LOCALAPPDATA%\Programs\sakshi`, added to your user `PATH`). Read them first if you like; they are short. Set `SAKSHI_VERSION=v0.1.0` to pin a version or `SAKSHI_INSTALL_DIR` to choose the folder.

**By hand:** release archives for macOS (Apple Silicon and Intel), Linux (x86-64 and ARM64, glibc) and Windows (x86-64 and ARM64) are attached to each [GitHub release](https://github.com/maxsignalhq/Sakshi/releases) with a `SHA256SUMS` file. Unpack one and put `sakshi` on your `PATH`.

> **macOS note:** release binaries are not yet notarized by Apple. The install script is unaffected, but if you download an archive with a browser, macOS may refuse to open `sakshi`. Clear the download flag once with `xattr -d com.apple.quarantine ./sakshi`.

**From source**, with a recent stable [Rust](https://rustup.rs):

```sh
cargo install --git https://github.com/maxsignalhq/Sakshi sakshi
```

Homebrew, Scoop and winget manifests are kept in [packaging/](packaging/). Publishing them is a manual step, described in [docs/RELEASING.md](docs/RELEASING.md); until that is done, use the archives or `cargo install`.

Optional: `git` on your `PATH` enables change tracking, and desktop notifications use your OS's own notification service.

## Use

```sh
sakshi                       # start your shell inside Sakshi
sakshi claude                # start Claude Code inside Sakshi
sakshi claude --resume       # arguments after the program go to the program
```

Sakshi shows a session list on the left and the selected session's terminal beside it. Agents run exactly as they would in any terminal; Sakshi only watches.

**Attached** (typing into a session): everything goes to the program. The one key Sakshi keeps is **Ctrl+]**, which detaches you back to the list.

**On the list:**

| Key | Does |
|-----|------|
| `up` / `down`, `j` / `k` | Select a session |
| `Enter`, `l`, `1`-`9` | Attach (the digit jumps straight to that session) |
| `n` | Start a new shell |
| `c` | Start Claude Code (shown only when it is installed) |
| `t` or `Tab` | Show or hide the details panel |
| `r` | Refresh the change summary |
| `x` | Close the session and kill its process (press twice) |
| `q` | Quit (press twice while sessions are running) |

Sessions keep running while you are on the list or in another session. They end when Sakshi exits.

### What it tells you

- **State** per session: `○` idle, `●` working, `!` needs you, `✓` done, `✗` failed. For Claude Code this is live, from its hooks. Other programs show only running or ended.
- **Notifications** when a session you are not looking at needs you, finishes or fails, or turns red. `--no-notify` turns them off.
- **Trust badge** (a coloured dot): green, yellow or red, with plain-English reasons in the details panel. See [TRUST.md](TRUST.md) for every rule.
- **While you were away**: re-attach to a session that did things while you were gone and Sakshi summarises them first.

### Looking back

Every session is recorded to a local log. Afterwards:

```sh
sakshi sessions                      # what ran, newest first, with state and trust
sakshi report                        # Markdown report of the latest session
sakshi report 0123abcd --format json # a specific session (any unique prefix of its id), as JSON
sakshi replay --speed 10             # replay the timeline at 10x
```

### Your own rules

Flag paths that matter in your codebase, or quiet ones that don't, with an optional `config.toml`:

```toml
[trust]
sensitive_paths = [{ pattern = "src/ledger/**", severity = "alert" }]
ignore_paths = ["fixtures/**"]
```

[TRUST.md](TRUST.md#your-own-rules) has the details and where the file lives on each OS.

## Claude Code

When Sakshi starts `claude`, it adds `--settings <file>` with hooks that report back to Sakshi. Your own Claude Code settings are never modified, and if Sakshi is not running the hooks do nothing and cannot slow the agent down. If the hooks ever go quiet while Claude Code is clearly busy, the session turns yellow with a `hooks-silent` finding, so you know the badge can't see everything. The details are in [PROTOCOL.md](PROTOCOL.md).

## Privacy

Everything stays on your machine. Sakshi has no accounts, no telemetry and makes no network calls. Hooks talk to Sakshi over a local Unix socket (owner-only permissions) or a Windows named pipe (remote clients rejected). Activity logs live in:

| OS | Location |
|----|----------|
| Windows | `%LOCALAPPDATA%\maxsignalhq\sakshi\data` |
| macOS | `~/Library/Application Support/dev.maxsignalhq.sakshi` |
| Linux | `~/.local/share/sakshi` |

Set `SAKSHI_DATA_DIR` to put them elsewhere, or pass `--no-log` to record nothing. Diagnostics go to `sakshi.log` in the same directory (`SAKSHI_LOG=debug` for more).

## Limits worth knowing

- Sessions do not survive Sakshi exiting or its terminal closing; there is no background daemon.
- Trust findings are pattern matches over commands, file paths and diffs. They will miss things and sometimes cry wolf. Treat a badge as a prompt to look.
- Change tracking needs a git repository. Work you had uncommitted before a session started is not blamed on it.
- Live state exists for Claude Code only. Adapters for other agents are the natural next step; the [protocol](PROTOCOL.md) is small on purpose, and [docs/ADAPTERS.md](docs/ADAPTERS.md) shows how to add one.

## Develop

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
python3 -m unittest discover -s packaging
```

Contributions are welcome: see [CONTRIBUTING.md](CONTRIBUTING.md), and please report vulnerabilities privately as described in [SECURITY.md](SECURITY.md). [SPEC.md](SPEC.md) describes the design and [CLAUDE.md](CLAUDE.md) the working rules. The workspace is `core` (model, events, trust engine; no UI or OS code), `pty`, `ipc`, `adapters`, `git`, `store`, `tui` and `cli`.

## License
[MIT](LICENSE)
