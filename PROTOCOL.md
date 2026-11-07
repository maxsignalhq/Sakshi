# Sakshi IPC protocol (v1)

Agents and their hooks report to a running Sakshi over a local endpoint.
Nothing uses the network.

## Transport

| Platform | Endpoint |
|----------|----------|
| macOS, Linux | Unix domain socket `ipc.sock` inside a private `sakshi-<pid>` directory (mode 0700, socket mode 0600) under `$XDG_RUNTIME_DIR` or the system temp directory |
| Windows | Named pipe `\\.\pipe\sakshi-<pid>` (remote clients rejected) |

Every Sakshi process listens on its own endpoint, so several can run at once.
Sakshi gives each session's child process two environment variables:

| Variable | Meaning |
|----------|---------|
| `SAKSHI_ENDPOINT` | Where to connect |
| `SAKSHI_SESSION_ID` | The session id to put in every message (16 lowercase hex digits) |

A process without these variables is not running under Sakshi and should send nothing.

## Framing

Newline-delimited JSON: one object per line, UTF-8, at most 1 MiB per line.
A client may send any number of lines on one connection, then close it.

```json
{"v":1,"session_id":"00000000000000ab","type":"state","ts":"2026-03-04T05:06:07Z","data":{"state":"working"}}
```

| Field | Type | Notes |
|-------|------|-------|
| `v` | integer | Protocol version. Must be `1`. |
| `session_id` | string | From `SAKSHI_SESSION_ID`. |
| `type` | string | One of the types below. |
| `ts` | string | RFC 3339 timestamp. |
| `data` | object | Fields depend on `type`. May be omitted only for types with no required fields (there are none today). |

Unknown extra fields are ignored, so newer senders stay compatible.

## Event types

| `type` | `data` |
|--------|--------|
| `state` | `{"state": "idle" \| "working" \| "waiting" \| "done" \| "error"}` |
| `tool_use` | `{"tool": string, "summary": string?}` |
| `file_edit` | `{"path": string}` |
| `command` | `{"command": string}` |
| `test_run` | `{"command": string, "passed": boolean?}` (`passed` omitted or `null` when unknown) |
| `message` | `{"text": string}` |
| `session` | `{"name": string, "agent": string, "cwd": string}` |
| `trust` | `{"badge": "green" \| "yellow" \| "red", "findings": [{"rule": string, "severity": "info" \| "warn" \| "alert", "reason": string}]}` |

`session` and `trust` are recorded by Sakshi itself, at the start of a session
and whenever its trust verdict changes (see TRUST.md). They appear in activity
logs; agents and hooks have no reason to send them.

States: `idle` started and not yet doing anything; `working` actively working;
`waiting` needs the user (a question, a permission prompt, or a finished turn);
`done` and `error` the session has ended. Ended sessions stay ended.

## Activity logs

Sakshi appends every event of a session, in this format, to
`sessions/<start time>-<session id>.jsonl` in its data directory. A log is
therefore a valid stream of protocol messages. Readers skip lines they cannot
parse, so logs written by a newer Sakshi still load.

## Errors

Malformed lines (bad JSON, wrong version, unknown type, missing fields, bad
session id or timestamp) are logged and skipped. The connection stays open.
A line over 1 MiB closes the connection.

## Hooks must fail open

If Sakshi is not running, is slow, or rejects a message, the sender must carry
on silently. `sakshi hook <agent>` does this: it prints nothing, exits 0, and
gives up after 500 ms.

## Claude Code

When Sakshi starts `claude`, it adds `--settings <file>` with a generated
settings file that routes these hook events to `sakshi hook claude`:
`SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`,
`PostToolUseFailure`, `PermissionRequest`, `Notification`, `Stop`, `SessionEnd`.
Your own Claude Code settings are not modified.

| Hook | Events sent |
|------|-------------|
| `SessionStart` | `state idle` |
| `UserPromptSubmit` | `state working` |
| `PreToolUse` | `state working`, `tool_use`, plus `command` for Bash and `file_edit` for Edit, Write, MultiEdit and NotebookEdit |
| `PostToolUse` | `test_run` (passed) when the Bash command is a recognised test runner |
| `PostToolUseFailure` | `test_run` (failed) when the Bash command is a recognised test runner |
| `PermissionRequest` | `state waiting`, `message` |
| `Notification` | `state waiting` for prompts that need you, and `message` |
| `Stop` | `state waiting` |
| `SessionEnd` | `state done` |

## Codex

Codex only runs hooks you have approved, so Sakshi cannot inject them for one
session as it does for Claude Code. Instead `sakshi setup codex` registers one
fixed plugin, through Codex's own CLI (`codex plugin marketplace add`,
`codex plugin add sakshi@sakshi`); `sakshi setup codex --remove` undoes it. You
then approve its hooks once in Codex ("Trust all and continue"). Until you do,
Codex skips them without a warning and the session shows `hooks-silent`.

The plugin routes `SessionStart`, `SessionEnd`, `UserPromptSubmit`,
`PreToolUse`, `PermissionRequest`, `PostToolUse`, `Stop` and `Interrupt` to
`sakshi hook codex`. The command does nothing unless `SAKSHI_ENDPOINT` is set
(only Sakshi sets it) and `sakshi` is on your path, and it always exits 0, so
Codex outside Sakshi is unaffected. The command finds `sakshi` by name, so
Sakshi puts the folder of the running `sakshi` first on the `PATH` it gives
Codex (unless that folder is already on it). A shell setup that overrides
`PATH` for hooks would defeat this.

By default Codex runs hooks inside a shared background app server that keeps
the environment it was first started with, so a session's hooks would never see
its own `SAKSHI_ENDPOINT` and `SAKSHI_SESSION_ID`. Sakshi therefore starts
Codex with `--no-daemon`, which gives the session a server of its own, started
by Sakshi, with Sakshi's environment. It adds the flag only if that Codex
accepts it, and not when you already passed `--no-daemon` or are connecting to a
server with `--remote`. A Codex too old to know the flag still starts, without
live state, and the reason is written to `sakshi.log`.

The plugin's files never change between Sakshi releases unless a release note
says so: Codex ties your approval to their exact contents, and a change asks
everyone to approve again.

| Hook | Events sent |
|------|-------------|
| `SessionStart` | `state idle` |
| `UserPromptSubmit` | `state working` |
| `PreToolUse` | `state working`, `tool_use`, plus `command` for shell tools, and `file_edit` for every file an `apply_patch` adds, updates, moves or deletes |
| `PostToolUse` | `test_run` when a shell command is a recognised test runner. `passed` is set only when the response states an exit code; otherwise it is left unknown |
| `PermissionRequest` | `state waiting`, `message` |
| `Stop`, `Interrupt` | `state waiting` |
| `SessionEnd` | `state done` |

Codex has no notification hook, so "needs you" comes from `PermissionRequest`
and from a finished or interrupted turn. Setup is not available on Windows yet:
the hook needs a POSIX shell.

