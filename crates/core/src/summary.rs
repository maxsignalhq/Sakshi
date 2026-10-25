//! "What happened?": a plain-English digest of a stretch of a session.

use time::OffsetDateTime;

use crate::event::{Event, EventKind};
use crate::session::SessionState;

/// Files named in a summary line before "and N more".
const MAX_FILES_NAMED: usize = 3;

/// What a session did over some span of time.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    /// Tool calls the agent made.
    pub tool_uses: usize,
    /// Distinct files edited, in the order first touched.
    pub files_edited: Vec<String>,
    /// Commands run, in order.
    pub commands: Vec<String>,
    /// Test runs that passed.
    pub tests_passed: usize,
    /// Test runs that failed.
    pub tests_failed: usize,
    /// Test runs with an unknown outcome.
    pub tests_unknown: usize,
    /// The latest message from the agent.
    pub last_message: Option<String>,
    /// The latest state the session reported.
    pub state: Option<SessionState>,
}

/// Summarise the events that happened after `since` (all events if `None`).
pub fn summarize(events: &[Event], since: Option<OffsetDateTime>) -> Summary {
    let mut summary = Summary::default();
    for event in events {
        if since.is_some_and(|since| event.ts <= since) {
            continue;
        }
        match &event.kind {
            EventKind::State(state) => summary.state = Some(*state),
            EventKind::ToolUse { .. } => summary.tool_uses += 1,
            EventKind::FileEdit { path } => {
                if !summary.files_edited.contains(path) {
                    summary.files_edited.push(path.clone());
                }
            }
            EventKind::Command { command } => summary.commands.push(command.clone()),
            EventKind::TestRun { passed, .. } => match passed {
                Some(true) => summary.tests_passed += 1,
                Some(false) => summary.tests_failed += 1,
                None => summary.tests_unknown += 1,
            },
            EventKind::Message { text } => summary.last_message = Some(text.clone()),
            EventKind::Session { .. } | EventKind::Trust { .. } => {}
        }
    }
    summary
}

impl Summary {
    /// Whether nothing worth reporting happened.
    pub fn is_quiet(&self) -> bool {
        self.tool_uses == 0
            && self.files_edited.is_empty()
            && self.commands.is_empty()
            && self.tests_passed + self.tests_failed + self.tests_unknown == 0
            && self.last_message.is_none()
            && self.state.is_none()
    }

    /// The summary as short sentences, one per line.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if self.tool_uses > 0 {
            lines.push(format!("Made {} tool call(s)", self.tool_uses));
        }
        if !self.files_edited.is_empty() {
            let shown: Vec<&str> = self
                .files_edited
                .iter()
                .take(MAX_FILES_NAMED)
                .map(String::as_str)
                .collect();
            let more = self.files_edited.len().saturating_sub(MAX_FILES_NAMED);
            let mut line = format!(
                "Edited {} file(s): {}",
                self.files_edited.len(),
                shown.join(", ")
            );
            if more > 0 {
                line.push_str(&format!(" and {more} more"));
            }
            lines.push(line);
        }
        if !self.commands.is_empty() {
            lines.push(format!("Ran {} command(s)", self.commands.len()));
        }
        let tests: Vec<String> = [
            (self.tests_passed, "passed"),
            (self.tests_failed, "failed"),
            (self.tests_unknown, "unclear"),
        ]
        .into_iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, what)| format!("{n} {what}"))
        .collect();
        if !tests.is_empty() {
            lines.push(format!("Test runs: {}", tests.join(", ")));
        }
        match self.state {
            Some(SessionState::Waiting) => {
                let mut line = String::from("Now waiting for you");
                if let Some(message) = &self.last_message {
                    line.push_str(&format!(": {message}"));
                }
                lines.push(line);
            }
            Some(SessionState::Done) => lines.push("Finished".to_string()),
            Some(SessionState::Error) => lines.push("Ended with an error".to_string()),
            Some(SessionState::Working) => lines.push("Still working".to_string()),
            Some(SessionState::Idle) | None => {}
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionId;

    fn at(secs: i64, kind: EventKind) -> Event {
        Event {
            session_id: SessionId(1),
            ts: OffsetDateTime::from_unix_timestamp(1_800_000_000 + secs).unwrap(),
            kind,
        }
    }

    fn t(secs: i64) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_800_000_000 + secs).unwrap()
    }

    fn edit(secs: i64, path: &str) -> Event {
        at(secs, EventKind::FileEdit { path: path.into() })
    }

    #[test]
    fn nothing_happened_is_quiet() {
        let s = summarize(&[], None);
        assert!(s.is_quiet());
        assert!(s.lines().is_empty());
    }

    #[test]
    fn counts_everything_without_a_cutoff() {
        let events = [
            at(
                0,
                EventKind::ToolUse {
                    tool: "Edit".into(),
                    summary: None,
                },
            ),
            edit(1, "a.rs"),
            edit(2, "b.rs"),
            edit(3, "a.rs"),
            at(
                4,
                EventKind::Command {
                    command: "ls".into(),
                },
            ),
            at(
                5,
                EventKind::TestRun {
                    command: "t".into(),
                    passed: Some(true),
                },
            ),
            at(
                6,
                EventKind::TestRun {
                    command: "t".into(),
                    passed: Some(false),
                },
            ),
            at(7, EventKind::Message { text: "hi".into() }),
            at(8, EventKind::State(SessionState::Waiting)),
        ];
        let s = summarize(&events, None);
        assert_eq!(s.tool_uses, 1);
        assert_eq!(s.files_edited, ["a.rs", "b.rs"]);
        assert_eq!(s.commands, ["ls"]);
        assert_eq!((s.tests_passed, s.tests_failed, s.tests_unknown), (1, 1, 0));
        assert_eq!(s.last_message.as_deref(), Some("hi"));
        assert_eq!(s.state, Some(SessionState::Waiting));
        assert!(!s.is_quiet());
    }

    #[test]
    fn only_events_after_the_cutoff_count() {
        let events = [edit(1, "old.rs"), edit(10, "new.rs"), edit(20, "newer.rs")];
        let s = summarize(&events, Some(t(10)));
        // The boundary itself counts as already seen.
        assert_eq!(s.files_edited, ["newer.rs"]);
        assert!(summarize(&events, Some(t(100))).is_quiet());
    }

    #[test]
    fn bookkeeping_events_are_not_activity() {
        let events = [at(
            0,
            EventKind::Session {
                name: "n".into(),
                agent: "a".into(),
                cwd: "c".into(),
            },
        )];
        assert!(summarize(&events, None).is_quiet());
    }

    #[test]
    fn lines_read_like_sentences() {
        let s = Summary {
            tool_uses: 4,
            files_edited: vec!["a".into(), "b".into(), "c".into(), "d".into(), "e".into()],
            commands: vec!["x".into(), "y".into()],
            tests_passed: 2,
            tests_failed: 1,
            tests_unknown: 0,
            last_message: Some("Permission needed: Bash rm x".into()),
            state: Some(SessionState::Waiting),
        };
        assert_eq!(
            s.lines(),
            [
                "Made 4 tool call(s)",
                "Edited 5 file(s): a, b, c and 2 more",
                "Ran 2 command(s)",
                "Test runs: 2 passed, 1 failed",
                "Now waiting for you: Permission needed: Bash rm x",
            ]
        );
    }

    #[test]
    fn end_states_have_their_own_lines() {
        let line = |state| {
            Summary {
                state: Some(state),
                ..Summary::default()
            }
            .lines()
        };
        assert_eq!(line(SessionState::Done), ["Finished"]);
        assert_eq!(line(SessionState::Error), ["Ended with an error"]);
        assert_eq!(line(SessionState::Working), ["Still working"]);
        assert!(line(SessionState::Idle).is_empty());
    }
}
