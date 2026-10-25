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

States: `idle` started and not yet doing anything; `working` actively working;
`waiting` needs the user (a question, a permission prompt, or a finished turn);
`done` and `error` the session has ended. Ended sessions stay ended.

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
