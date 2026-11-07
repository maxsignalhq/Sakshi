//! Claude Code: hooks injected with `--settings`, payloads from hook stdin.

use std::io;
use std::path::Path;
use std::process::{Command, Stdio};

use sakshi_core::{is_test_command, AgentKind, Event, EventKind, SessionId, SessionState};
use serde_json::{json, Value};

use crate::{AgentAdapter, LaunchContext};

/// Hook events Sakshi subscribes to.
const HOOK_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "Notification",
    "Stop",
    "SessionEnd",
];

/// Hook timeout in seconds. Hooks must never hold the agent up.
const HOOK_TIMEOUT_SECS: u32 = 5;

/// Longest text kept in an event field.
const MAX_TEXT_CHARS: usize = 1000;

/// Adapter for Claude Code.
pub struct ClaudeAdapter;

impl AgentAdapter for ClaudeAdapter {
    fn kind(&self) -> AgentKind {
        AgentKind::Claude
    }

    fn display_name(&self) -> &'static str {
        "Claude Code"
    }

    fn default_program(&self) -> &'static str {
        "claude"
    }

    fn matches(&self, program: &str) -> bool {
        Path::new(program)
            .file_stem()
            .is_some_and(|stem| stem.eq_ignore_ascii_case("claude"))
    }

    fn probe(&self) -> bool {
        Command::new("claude")
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn launch_args(&self, ctx: &LaunchContext<'_>) -> io::Result<Vec<String>> {
        let settings = settings_json(ctx.hook_exe);
        let path = ctx.runtime_dir.join("claude-settings.json");
        std::fs::write(&path, settings.to_string())?;
        Ok(vec![
            "--settings".to_string(),
            path.to_string_lossy().into_owned(),
        ])
    }

    fn translate_hook(&self, session_id: SessionId, payload: &Value) -> Vec<Event> {
        translate(payload)
            .into_iter()
            .map(|kind| Event::now(session_id, kind))
            .collect()
    }
}

/// The `--settings` document that routes every hook to `sakshi hook claude`.
///
/// Uses exec form (`command` plus `args`), so no shell is involved and paths
/// with spaces or backslashes need no quoting.
fn settings_json(hook_exe: &Path) -> Value {
    let hook = json!({
        "type": "command",
        "command": hook_exe.to_string_lossy(),
        "args": ["hook", "claude"],
        "timeout": HOOK_TIMEOUT_SECS,
    });
    let hooks: serde_json::Map<String, Value> = HOOK_EVENTS
        .iter()
        .map(|name| ((*name).to_string(), json!([{ "hooks": [hook.clone()] }])))
        .collect();
    json!({ "hooks": hooks })
}

fn truncate(text: &str) -> String {
    if text.chars().count() <= MAX_TEXT_CHARS {
        text.to_string()
    } else {
        let mut cut: String = text.chars().take(MAX_TEXT_CHARS).collect();
        cut.push('…');
        cut
    }
}

fn str_at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut cur = value;
    for key in path {
        cur = cur.get(key)?;
    }
    cur.as_str()
}

/// Path of the file a file-editing tool call targets.
fn edited_path(tool_input: &Value) -> Option<&str> {
    str_at(tool_input, &["file_path"]).or_else(|| str_at(tool_input, &["notebook_path"]))
}

fn is_edit_tool(tool: &str) -> bool {
    matches!(tool, "Edit" | "Write" | "MultiEdit" | "NotebookEdit")
}

/// One-line description of a tool call for the activity feed.
fn tool_summary(tool: &str, input: &Value) -> Option<String> {
    let text = if tool == "Bash" {
        str_at(input, &["description"]).or_else(|| str_at(input, &["command"]))
    } else if is_edit_tool(tool) {
        edited_path(input)
    } else {
        str_at(input, &["file_path"])
            .or_else(|| str_at(input, &["pattern"]))
            .or_else(|| str_at(input, &["url"]))
            .or_else(|| str_at(input, &["description"]))
    };
    text.map(truncate)
}

fn translate(payload: &Value) -> Vec<EventKind> {
    let Some(name) = str_at(payload, &["hook_event_name"]) else {
        return Vec::new();
    };
    let tool = str_at(payload, &["tool_name"]).unwrap_or("");
    let input = payload.get("tool_input").unwrap_or(&Value::Null);
    let bash_command = if tool == "Bash" {
        str_at(input, &["command"])
    } else {
        None
    };

    match name {
        "SessionStart" => vec![EventKind::State(SessionState::Idle)],
        "UserPromptSubmit" => vec![EventKind::State(SessionState::Working)],
        "PreToolUse" => {
            // A tool starting means any pending prompt was answered.
            let mut events = vec![
                EventKind::State(SessionState::Working),
                EventKind::ToolUse {
                    tool: tool.to_string(),
                    summary: tool_summary(tool, input),
                },
            ];
            if let Some(command) = bash_command {
                events.push(EventKind::Command {
                    command: truncate(command),
                });
            }
            if is_edit_tool(tool) {
                if let Some(path) = edited_path(input) {
                    events.push(EventKind::FileEdit {
                        path: path.to_string(),
                    });
                }
            }
            events
        }
        "PostToolUse" => match bash_command {
            Some(command) if is_test_command(command) => {
                let interrupted = payload
                    .get("tool_response")
                    .and_then(|r| r.get("interrupted"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                vec![EventKind::TestRun {
                    command: truncate(command),
                    passed: if interrupted { None } else { Some(true) },
                }]
            }
            _ => Vec::new(),
        },
        "PostToolUseFailure" => match bash_command {
            Some(command) if is_test_command(command) => {
                let interrupted = payload
                    .get("is_interrupt")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                vec![EventKind::TestRun {
                    command: truncate(command),
                    passed: if interrupted { None } else { Some(false) },
                }]
            }
            _ => Vec::new(),
        },
        "PermissionRequest" => {
            let what = tool_summary(tool, input).unwrap_or_default();
            vec![
                EventKind::State(SessionState::Waiting),
                EventKind::Message {
                    text: truncate(format!("Permission needed: {tool} {what}").trim()),
                },
            ]
        }
        "Notification" => {
            let mut events = Vec::new();
            let needs_user = match str_at(payload, &["notification_type"]) {
                Some(kind) => matches!(
                    kind,
                    "permission_prompt"
                        | "idle_prompt"
                        | "elicitation_dialog"
                        | "agent_needs_input"
                ),
                None => true,
            };
            if needs_user {
                events.push(EventKind::State(SessionState::Waiting));
            }
            if let Some(message) = str_at(payload, &["message"]) {
                events.push(EventKind::Message {
                    text: truncate(message),
                });
            }
            events
        }
        "Stop" => vec![EventKind::State(SessionState::Waiting)],
        "SessionEnd" => vec![EventKind::State(SessionState::Done)],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(payload: Value) -> Vec<EventKind> {
        translate(&payload)
    }

    #[test]
    fn lifecycle_hooks_map_to_states() {
        let state = |name: &str| kinds(json!({ "hook_event_name": name }));
        assert_eq!(
            state("SessionStart"),
            [EventKind::State(SessionState::Idle)]
        );
        assert_eq!(
            state("UserPromptSubmit"),
            [EventKind::State(SessionState::Working)]
        );
        assert_eq!(state("Stop"), [EventKind::State(SessionState::Waiting)]);
        assert_eq!(state("SessionEnd"), [EventKind::State(SessionState::Done)]);
    }

    #[test]
    fn unknown_or_malformed_payloads_yield_nothing() {
        assert!(kinds(json!({ "hook_event_name": "PreCompact" })).is_empty());
        assert!(kinds(json!({})).is_empty());
        assert!(kinds(json!("text")).is_empty());
        assert!(kinds(json!(null)).is_empty());
    }

    #[test]
    fn bash_pre_tool_use_reports_the_command() {
        let events = kinds(json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": { "command": "rm -rf build", "description": "Clean" }
        }));
        assert_eq!(events[0], EventKind::State(SessionState::Working));
        assert_eq!(
            events[1],
            EventKind::ToolUse {
                tool: "Bash".into(),
                summary: Some("Clean".into())
            }
        );
        assert_eq!(
            events[2],
            EventKind::Command {
                command: "rm -rf build".into()
            }
        );
    }

    #[test]
    fn edit_tools_report_file_edits() {
        for (tool, key) in [
            ("Edit", "file_path"),
            ("Write", "file_path"),
            ("MultiEdit", "file_path"),
            ("NotebookEdit", "notebook_path"),
        ] {
            let events = kinds(json!({
                "hook_event_name": "PreToolUse",
                "tool_name": tool,
                "tool_input": { key: "/p/a.rs" }
            }));
            assert!(
                events.contains(&EventKind::FileEdit {
                    path: "/p/a.rs".into()
                }),
                "{tool}"
            );
        }
    }

    #[test]
    fn read_only_tools_report_no_edit() {
        let events = kinds(json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Read",
            "tool_input": { "file_path": "/p/a.rs" }
        }));
        assert!(!events
            .iter()
            .any(|e| matches!(e, EventKind::FileEdit { .. })));
        assert!(
            matches!(&events[1], EventKind::ToolUse { summary: Some(s), .. } if s == "/p/a.rs")
        );
    }

    #[test]
    fn test_runs_are_reported_with_outcome() {
        let ok = kinds(json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash",
            "tool_input": { "command": "cargo test" },
            "tool_response": { "interrupted": false }
        }));
        assert_eq!(
            ok,
            [EventKind::TestRun {
                command: "cargo test".into(),
                passed: Some(true)
            }]
        );
        let failed = kinds(json!({
            "hook_event_name": "PostToolUseFailure",
            "tool_name": "Bash",
            "tool_input": { "command": "cargo test" },
            "error": "Exit code 101",
            "is_interrupt": false
        }));
        assert_eq!(
            failed,
            [EventKind::TestRun {
                command: "cargo test".into(),
                passed: Some(false)
            }]
        );
        let interrupted = kinds(json!({
            "hook_event_name": "PostToolUseFailure",
            "tool_name": "Bash",
            "tool_input": { "command": "cargo test" },
            "is_interrupt": true
        }));
        assert_eq!(
            interrupted,
            [EventKind::TestRun {
                command: "cargo test".into(),
                passed: None
            }]
        );
    }

    #[test]
    fn non_test_post_tool_use_is_silent() {
        assert!(kinds(json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash",
            "tool_input": { "command": "ls" }
        }))
        .is_empty());
    }

    #[test]
    fn permission_request_and_notifications_wait_for_the_user() {
        let events = kinds(json!({
            "hook_event_name": "PermissionRequest",
            "tool_name": "Bash",
            "tool_input": { "command": "rm -rf node_modules" }
        }));
        assert_eq!(events[0], EventKind::State(SessionState::Waiting));
        assert!(matches!(&events[1], EventKind::Message { text } if text.contains("Bash")));

        let events = kinds(json!({
            "hook_event_name": "Notification",
            "notification_type": "idle_prompt",
            "message": "Claude is waiting for your input"
        }));
        assert_eq!(events[0], EventKind::State(SessionState::Waiting));
    }

    #[test]
    fn informational_notifications_do_not_change_state() {
        let events = kinds(json!({
            "hook_event_name": "Notification",
            "notification_type": "auth_success",
            "message": "Signed in"
        }));
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], EventKind::Message { .. }));
    }

    #[test]
    fn long_text_is_truncated() {
        let long = "x".repeat(5000);
        let events = kinds(json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": { "command": long }
        }));
        match &events[2] {
            EventKind::Command { command } => {
                assert_eq!(command.chars().count(), MAX_TEXT_CHARS + 1);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn settings_cover_every_event_in_exec_form() {
        let settings = settings_json(Path::new("/opt/my tools/sakshi"));
        let hooks = settings["hooks"].as_object().unwrap();
        assert_eq!(hooks.len(), HOOK_EVENTS.len());
        for name in HOOK_EVENTS {
            let hook = &hooks[*name][0]["hooks"][0];
            assert_eq!(hook["type"], "command");
            assert_eq!(hook["command"], "/opt/my tools/sakshi");
            assert_eq!(hook["args"], json!(["hook", "claude"]));
        }
    }

    #[test]
    fn launch_args_write_the_settings_file() {
        let dir = std::env::temp_dir().join(format!("sakshi-claude-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ctx = LaunchContext {
            hook_exe: Path::new("sakshi"),
            runtime_dir: &dir,
            program: "claude",
            user_args: &[],
        };
        let args = ClaudeAdapter.launch_args(&ctx).unwrap();
        assert_eq!(args[0], "--settings");
        let written = std::fs::read_to_string(&args[1]).unwrap();
        assert!(serde_json::from_str::<Value>(&written).is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn matches_program_names() {
        assert!(ClaudeAdapter.matches("claude"));
        assert!(ClaudeAdapter.matches("claude.exe"));
        // Backslashes only separate path components on Windows.
        #[cfg(windows)]
        assert!(ClaudeAdapter.matches("C:\\Users\\me\\.local\\bin\\claude.exe"));
        #[cfg(not(windows))]
        assert!(ClaudeAdapter.matches("/home/me/.local/bin/claude"));
        assert!(!ClaudeAdapter.matches("claude-code-router"));
    }
}
