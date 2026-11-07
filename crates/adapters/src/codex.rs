//! OpenAI Codex: hooks from a plugin registered once by `sakshi setup codex`,
//! payloads from hook stdin.
//!
//! Codex runs only hooks the user has approved, so nothing can be injected for
//! a single session (see [`crate::codex_plugin`]). The plugin's hooks call
//! `sakshi hook codex` whenever Codex runs inside a Sakshi session.
//!
//! The hooks find their session through `SAKSHI_*` environment variables, but
//! by default Codex runs hooks inside a shared background app server that keeps
//! whatever environment it was first started with. Every session's hooks would
//! then see a stale session (or none) and nothing would reach Sakshi. So the
//! adapter launches Codex with `--no-daemon`, which gives the session its own
//! server, a child of the process Sakshi started, with Sakshi's environment.

use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};

use sakshi_core::{is_test_command, AgentKind, Event, EventKind, SessionId, SessionState};
use serde_json::Value;

use crate::{AgentAdapter, LaunchContext};

/// Longest text kept in an event field.
const MAX_TEXT_CHARS: usize = 1000;

/// Adapter for OpenAI Codex.
pub struct CodexAdapter;

impl AgentAdapter for CodexAdapter {
    fn kind(&self) -> AgentKind {
        AgentKind::Codex
    }

    fn display_name(&self) -> &'static str {
        "Codex"
    }

    fn default_program(&self) -> &'static str {
        "codex"
    }

    /// `c` belongs to Claude Code and `x` closes a session.
    fn start_key(&self) -> char {
        'o'
    }

    fn matches(&self, program: &str) -> bool {
        Path::new(program)
            .file_stem()
            .is_some_and(|stem| stem.eq_ignore_ascii_case("codex"))
    }

    fn probe(&self) -> bool {
        Command::new("codex")
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn launch_args(&self, ctx: &LaunchContext<'_>) -> io::Result<Vec<String>> {
        Ok(launch_flags(ctx.user_args, || {
            supports_no_daemon(ctx.program)
        }))
    }

    fn translate_hook(&self, session_id: SessionId, payload: &Value) -> Vec<Event> {
        translate(payload)
            .into_iter()
            .map(|kind| Event::now(session_id, kind))
            .collect()
    }
}

/// Flag that makes Codex use its own app server instead of the shared one.
const NO_DAEMON: &str = "--no-daemon";

/// The flags to put before the user's arguments.
///
/// Nothing is added if the user already passed `--no-daemon` (Codex rejects it
/// twice) or is connecting to a remote server (its hooks are out of reach
/// either way), or if this Codex does not know the flag.
fn launch_flags(user_args: &[String], supported: impl FnOnce() -> bool) -> Vec<String> {
    let given = user_args
        .iter()
        .any(|a| a == NO_DAEMON || a == "--remote" || a.starts_with("--remote="));
    if !given && supported() {
        vec![NO_DAEMON.to_string()]
    } else {
        Vec::new()
    }
}

/// Whether `program` accepts `--no-daemon`, learned by running it once.
///
/// An older Codex exits non-zero on an unknown flag, which would stop it
/// starting at all, so any doubt means "no". Answers are kept per program
/// because a launch should not pay for the probe each time.
fn supports_no_daemon(program: &str) -> bool {
    static CACHE: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Mutex::default);
    if let Some(known) = cache
        .lock()
        .ok()
        .and_then(|known| known.get(program).copied())
    {
        return known;
    }
    let supported = Command::new(program)
        .args([NO_DAEMON, "--version"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if let Ok(mut known) = cache.lock() {
        known.insert(program.to_string(), supported);
    }
    supported
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

fn str_at<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key)?.as_str()
}

/// Files an `apply_patch` call touches, from its patch text.
///
/// The patch is Codex's own format: `*** Add File: path`, `*** Update File:
/// path`, `*** Delete File: path`, and `*** Move to: path` after an update.
fn patched_paths(patch: &str) -> Vec<String> {
    const MARKERS: [&str; 4] = [
        "*** Add File: ",
        "*** Update File: ",
        "*** Delete File: ",
        "*** Move to: ",
    ];
    let mut paths: Vec<String> = Vec::new();
    for line in patch.lines() {
        for marker in MARKERS {
            if let Some(path) = line.strip_prefix(marker) {
                let path = path.trim();
                if !path.is_empty() && !paths.iter().any(|p| p == path) {
                    paths.push(path.to_string());
                }
            }
        }
    }
    paths
}

/// The command or patch a tool call carries, wherever this Codex version puts it.
fn tool_command(input: &Value) -> Option<&str> {
    str_at(input, "command").or_else(|| str_at(input, "input"))
}

/// Exit code from a tool response, when the response states one
/// (`Exit code: 0` on its first line). Shell responses often do not, and then
/// the outcome is unknown rather than guessed.
fn exit_code(response: &Value) -> Option<i64> {
    let text = response
        .as_str()
        .or_else(|| str_at(response, "output"))
        .or_else(|| str_at(response, "stdout"))?;
    let rest = text.trim_start().strip_prefix("Exit code:")?;
    rest.split_whitespace().next()?.parse().ok()
}

fn is_shell(tool: &str) -> bool {
    matches!(tool, "Bash" | "shell" | "local_shell" | "exec_command")
}

fn translate(payload: &Value) -> Vec<EventKind> {
    let Some(name) = str_at(payload, "hook_event_name") else {
        return Vec::new();
    };
    let tool = str_at(payload, "tool_name").unwrap_or("");
    let input = payload.get("tool_input").unwrap_or(&Value::Null);
    let command = tool_command(input);
    let shell_command = if is_shell(tool) { command } else { None };

    match name {
        "SessionStart" => vec![EventKind::State(SessionState::Idle)],
        "UserPromptSubmit" => vec![EventKind::State(SessionState::Working)],
        "PreToolUse" => {
            // A tool starting means any pending prompt was answered.
            let mut events = vec![EventKind::State(SessionState::Working)];
            if tool == "apply_patch" {
                let paths = command.map(patched_paths).unwrap_or_default();
                events.push(EventKind::ToolUse {
                    tool: tool.to_string(),
                    summary: if paths.is_empty() {
                        None
                    } else {
                        Some(truncate(&paths.join(", ")))
                    },
                });
                events.extend(paths.into_iter().map(|path| EventKind::FileEdit { path }));
            } else {
                events.push(EventKind::ToolUse {
                    tool: tool.to_string(),
                    summary: shell_command.map(truncate),
                });
                if let Some(command) = shell_command {
                    events.push(EventKind::Command {
                        command: truncate(command),
                    });
                }
            }
            events
        }
        "PostToolUse" => match shell_command {
            Some(command) if is_test_command(command) => vec![EventKind::TestRun {
                command: truncate(command),
                passed: payload
                    .get("tool_response")
                    .and_then(exit_code)
                    .map(|code| code == 0),
            }],
            _ => Vec::new(),
        },
        "PermissionRequest" => {
            let what = command.map(truncate).unwrap_or_default();
            vec![
                EventKind::State(SessionState::Waiting),
                EventKind::Message {
                    text: truncate(format!("Permission needed: {tool} {what}").trim()),
                },
            ]
        }
        // A finished or interrupted turn hands control back to the user.
        "Stop" | "Interrupt" => vec![EventKind::State(SessionState::Waiting)],
        "SessionEnd" => vec![EventKind::State(SessionState::Done)],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn kinds(payload: Value) -> Vec<EventKind> {
        translate(&payload)
    }

    // Payload shapes below are real ones captured from Codex 0.166 (paths shortened).

    #[test]
    fn lifecycle_hooks_map_to_states() {
        let state =
            |name: &str| kinds(json!({"hook_event_name": name, "session_id": "s", "cwd": "/w"}));
        assert_eq!(
            state("SessionStart"),
            [EventKind::State(SessionState::Idle)]
        );
        assert_eq!(
            state("UserPromptSubmit"),
            [EventKind::State(SessionState::Working)]
        );
        assert_eq!(state("Stop"), [EventKind::State(SessionState::Waiting)]);
        assert_eq!(
            state("Interrupt"),
            [EventKind::State(SessionState::Waiting)]
        );
        assert_eq!(state("SessionEnd"), [EventKind::State(SessionState::Done)]);
    }

    #[test]
    fn unknown_or_malformed_payloads_yield_nothing() {
        assert!(kinds(json!({})).is_empty());
        assert!(kinds(json!("text")).is_empty());
        assert!(kinds(json!({"hook_event_name": "PreCompact"})).is_empty());
        assert!(kinds(json!({"hook_event_name": 7})).is_empty());
    }

    #[test]
    fn a_shell_command_is_reported_as_working_and_a_command() {
        let events = kinds(json!({
            "hook_event_name": "PreToolUse", "tool_name": "Bash",
            "tool_input": {"command": "printf hi > x.txt"}, "permission_mode": "default"
        }));
        assert_eq!(
            events,
            [
                EventKind::State(SessionState::Working),
                EventKind::ToolUse {
                    tool: "Bash".into(),
                    summary: Some("printf hi > x.txt".into())
                },
                EventKind::Command {
                    command: "printf hi > x.txt".into()
                },
            ]
        );
    }

    #[test]
    fn apply_patch_reports_every_file_it_touches() {
        let patch = "*** Begin Patch\n*** Add File: /w/new.py\n+print(1)\n*** Update File: /w/a.py\n*** Move to: /w/b.py\n@@\n-x\n+y\n*** Delete File: /w/old.py\n*** End Patch";
        let events = kinds(json!({
            "hook_event_name": "PreToolUse", "tool_name": "apply_patch",
            "tool_input": {"command": patch}
        }));
        let edited: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                EventKind::FileEdit { path } => Some(path.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(edited, ["/w/new.py", "/w/a.py", "/w/b.py", "/w/old.py"]);
        // A patch is not a shell command.
        assert!(!events
            .iter()
            .any(|e| matches!(e, EventKind::Command { .. })));
    }

    #[test]
    fn an_empty_or_odd_patch_reports_no_edit_and_does_not_panic() {
        for input in [
            json!({}),
            json!({"command": ""}),
            json!({"command": "*** Add File: "}),
            json!(null),
        ] {
            let events = kinds(
                json!({"hook_event_name": "PreToolUse", "tool_name": "apply_patch", "tool_input": input}),
            );
            assert!(
                !events
                    .iter()
                    .any(|e| matches!(e, EventKind::FileEdit { .. })),
                "{events:?}"
            );
        }
    }

    #[test]
    fn other_tools_report_use_but_no_command_or_edit() {
        let events = kinds(json!({
            "hook_event_name": "PreToolUse", "tool_name": "mcp__docs__search",
            "tool_input": {"command": "rm -rf /"}
        }));
        // Only a shell tool's input is a command; never treat other tools' text as one.
        assert!(!events
            .iter()
            .any(|e| matches!(e, EventKind::Command { .. } | EventKind::FileEdit { .. })));
        assert!(events
            .iter()
            .any(|e| matches!(e, EventKind::ToolUse { tool, .. } if tool == "mcp__docs__search")));
    }

    #[test]
    fn a_test_run_with_no_stated_exit_code_has_an_unknown_outcome() {
        // Real response of a failed `cargo test`: output only, no exit code.
        let events = kinds(json!({
            "hook_event_name": "PostToolUse", "tool_name": "Bash",
            "tool_input": {"command": "cargo test ."},
            "tool_response": "error: could not find `Cargo.toml` in `/w` or any parent directory\n"
        }));
        assert_eq!(
            events,
            [EventKind::TestRun {
                command: "cargo test .".into(),
                passed: None
            }]
        );
    }

    #[test]
    fn a_stated_exit_code_decides_the_test_outcome() {
        let run = |code: i64| {
            kinds(json!({
                "hook_event_name": "PostToolUse", "tool_name": "Bash",
                "tool_input": {"command": "pytest -q"},
                "tool_response": format!("Exit code: {code}\nWall time: 1 seconds\nOutput:\n...")
            }))
        };
        assert_eq!(
            run(0),
            [EventKind::TestRun {
                command: "pytest -q".into(),
                passed: Some(true)
            }]
        );
        assert_eq!(
            run(1),
            [EventKind::TestRun {
                command: "pytest -q".into(),
                passed: Some(false)
            }]
        );
    }

    #[test]
    fn post_tool_use_is_silent_for_everything_that_is_not_a_test() {
        for (tool, command) in [
            ("Bash", "ls -la"),
            ("apply_patch", "*** Begin Patch"),
            ("Bash", "echo cargo test"),
        ] {
            let events = kinds(json!({
                "hook_event_name": "PostToolUse", "tool_name": tool,
                "tool_input": {"command": command}, "tool_response": "Exit code: 0\n"
            }));
            assert!(events.is_empty(), "{tool} {command}: {events:?}");
        }
    }

    #[test]
    fn a_permission_request_waits_for_the_user() {
        let events = kinds(json!({
            "hook_event_name": "PermissionRequest", "tool_name": "Bash",
            "tool_input": {"command": "printf hi > x.txt", "description": "Allow creating x.txt"}
        }));
        assert_eq!(events[0], EventKind::State(SessionState::Waiting));
        assert!(
            matches!(&events[1], EventKind::Message { text } if text == "Permission needed: Bash printf hi > x.txt")
        );
    }

    #[test]
    fn long_text_is_truncated() {
        let long = "a".repeat(5000);
        let events = kinds(json!({
            "hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": {"command": long}
        }));
        for e in events {
            if let EventKind::Command { command } = e {
                assert!(command.chars().count() <= MAX_TEXT_CHARS + 1);
            }
        }
    }

    #[test]
    fn matches_program_names() {
        let a = CodexAdapter;
        assert!(a.matches("codex"));
        assert!(a.matches("/opt/homebrew/bin/codex"));
        assert!(a.matches("codex.exe"));
        assert!(!a.matches("codex-cli-helper"));
        assert!(!a.matches("claude"));
    }

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn launch(program: &str, user_args: &[String]) -> Vec<String> {
        let ctx = LaunchContext {
            hook_exe: Path::new("sakshi"),
            runtime_dir: Path::new("."),
            program,
            user_args,
        };
        CodexAdapter.launch_args(&ctx).unwrap()
    }

    #[test]
    fn uses_its_own_start_key() {
        assert_eq!(CodexAdapter.start_key(), 'o');
    }

    // Codex runs hooks in a shared background server that keeps the environment
    // it started with, so a session's hooks never see its SAKSHI_* variables.
    // `--no-daemon` keeps the session's own server, which they do see.
    #[test]
    fn runs_codex_without_the_shared_server() {
        assert_eq!(launch("true", &[]), args(&["--no-daemon"]));
    }

    #[test]
    fn adds_nothing_when_codex_does_not_know_the_flag() {
        // `false` stands in for a Codex that rejects an unknown flag.
        assert!(launch("false", &[]).is_empty());
    }

    #[test]
    fn adds_nothing_when_codex_cannot_be_run() {
        assert!(launch("sakshi-no-such-program-x", &[]).is_empty());
    }

    // Codex refuses `--no-daemon` given twice, which would stop it starting.
    #[test]
    fn does_not_repeat_a_flag_the_user_gave() {
        assert!(launch("true", &args(&["--no-daemon"])).is_empty());
        assert!(launch("true", &args(&["resume", "--no-daemon"])).is_empty());
    }

    // With --remote the hooks run on the remote server, out of reach either way.
    #[test]
    fn leaves_remote_sessions_alone() {
        assert!(launch("true", &args(&["--remote", "unix://"])).is_empty());
        assert!(launch("true", &args(&["--remote=ws://h:1"])).is_empty());
    }

    #[test]
    fn passes_other_arguments_through_unchanged() {
        assert_eq!(
            launch("true", &args(&["resume", "--last"])),
            args(&["--no-daemon"])
        );
    }
}
