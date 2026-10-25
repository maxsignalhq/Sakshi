//! Opt-in tests against a real, logged-in `claude`. Each makes one tiny request
//! through your own Claude Code account, so they never run by default:
//!
//! ```sh
//! cargo test -p sakshi --test claude_real -- --ignored --nocapture
//! SAKSHI_TEST_BIN=/path/to/released/sakshi cargo test -p sakshi --test claude_real -- --ignored
//! ```
//!
//! They check the whole chain nothing else can: Sakshi launches the agent with
//! hooks injected through `--settings`, the real agent's hooks call back into
//! `sakshi hook claude`, and the resulting events land in the activity log.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use common::{DataDir, Tui};
use sakshi_core::{Event, EventKind, SessionState};

/// Run `claude <claude_args>` inside Sakshi to completion and return what the
/// activity log recorded. `expect_on_screen` must appear in claude's answer.
async fn run_claude(tag: &str, claude_args: &[&str], expect_on_screen: &str) -> Vec<Event> {
    let data = DataDir::new(tag);
    let mut args = vec!["--no-notify", "claude"];
    args.extend_from_slice(claude_args);
    let mut tui = Tui::start(&args, &data.0);

    // The agent answers on its own terminal inside Sakshi...
    tui.wait_for_within("claude's answer", Duration::from_secs(180), |s| {
        s.contains(expect_on_screen)
    })
    .await;
    // ...and, being a one-shot `-p` run, exits.
    tui.wait_for_within("the exit notice", Duration::from_secs(90), |s| {
        s.contains("Process exited")
    })
    .await;

    // Any key returns to the list, where nothing is running, so `q` quits.
    tui.send(b"x");
    tui.wait_for("the list", |s| s.contains("Enter attach"))
        .await;
    tui.send(b"q");
    assert!(tui.wait_for_exit().await, "sakshi should exit cleanly");

    let logs = sakshi_store::list_logs(&sakshi_store::sessions_dir(&data.0)).unwrap();
    assert_eq!(logs.len(), 1, "{logs:?}");
    let events = sakshi_store::read_events(&logs[0].path).unwrap();
    eprintln!("recorded events:");
    for event in &events {
        eprintln!("  {:?}", event.kind);
    }
    events
}

fn kinds(events: &[Event]) -> Vec<&EventKind> {
    events.iter().map(|e| &e.kind).collect()
}

#[tokio::test]
#[ignore = "needs a logged-in claude and makes one real request; run with --ignored"]
async fn a_real_claude_reports_its_state_through_the_hooks() {
    let events = run_claude(
        "state",
        &["-p", "Reply with exactly the word: pong"],
        "pong",
    )
    .await;
    let kinds = kinds(&events);

    assert!(
        matches!(kinds.first(), Some(EventKind::Session { agent, .. }) if agent == "claude"),
        "the session should be recorded as a claude session"
    );
    // These can only have come from the real agent's hooks.
    assert!(
        kinds.contains(&&EventKind::State(SessionState::Working)),
        "no 'working' state arrived from the UserPromptSubmit hook"
    );
    assert!(
        kinds.contains(&&EventKind::State(SessionState::Waiting))
            || kinds.contains(&&EventKind::State(SessionState::Done)),
        "no end-of-turn state arrived from the Stop or SessionEnd hook"
    );
}

#[tokio::test]
#[ignore = "needs a logged-in claude and makes one real request; run with --ignored"]
async fn a_real_claude_reports_the_commands_it_runs() {
    let events = run_claude(
        "tools",
        &[
            "-p",
            "Use the Bash tool to run exactly this command and then say done: echo sakshi-real-test",
            "--allowedTools",
            "Bash(echo:*)",
        ],
        "done",
    )
    .await;
    let kinds = kinds(&events);

    assert!(
        kinds
            .iter()
            .any(|k| matches!(k, EventKind::ToolUse { tool, .. } if tool == "Bash")),
        "no Bash tool use arrived from the PreToolUse hook"
    );
    assert!(
        kinds
            .iter()
            .any(|k| matches!(k, EventKind::Command { command } if command.contains("echo sakshi-real-test"))),
        "the command the agent ran was not recorded"
    );
}
