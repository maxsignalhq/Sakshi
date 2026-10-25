use time::OffsetDateTime;

use crate::session::{SessionId, SessionState};
use crate::trust::{Badge, Finding};

/// What happened in a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventKind {
    /// The session changed state.
    State(SessionState),
    /// The agent used a tool.
    ToolUse {
        /// Tool name, e.g. `Bash` or `Edit`.
        tool: String,
        /// Short human-readable description of the call, if any.
        summary: Option<String>,
    },
    /// The agent edited or wrote a file.
    FileEdit {
        /// Path of the file, as the agent reported it.
        path: String,
    },
    /// The agent ran a shell command.
    Command {
        /// The command line.
        command: String,
    },
    /// The agent ran a test command.
    TestRun {
        /// The command line.
        command: String,
        /// `Some(true)` on success, `Some(false)` on failure, `None` if unknown.
        passed: Option<bool>,
    },
    /// Free-form text from the agent or the adapter.
    Message {
        /// The text.
        text: String,
    },
    /// Recorded by Sakshi when a session starts, so a log describes itself.
    Session {
        /// Display name of the session.
        name: String,
        /// Which agent it runs, e.g. `claude` or `generic`.
        agent: String,
        /// Working directory it started in.
        cwd: String,
    },
    /// Recorded by Sakshi whenever a session's trust verdict changes.
    Trust {
        /// The overall verdict.
        badge: Badge,
        /// The findings behind it, worst first.
        findings: Vec<Finding>,
    },
}

impl EventKind {
    /// The protocol `type` name of this kind.
    pub fn type_name(&self) -> &'static str {
        match self {
            EventKind::State(_) => "state",
            EventKind::ToolUse { .. } => "tool_use",
            EventKind::FileEdit { .. } => "file_edit",
            EventKind::Command { .. } => "command",
            EventKind::TestRun { .. } => "test_run",
            EventKind::Message { .. } => "message",
            EventKind::Session { .. } => "session",
            EventKind::Trust { .. } => "trust",
        }
    }
}

/// A typed, timestamped event belonging to one session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    /// The session the event belongs to.
    pub session_id: SessionId,
    /// When it happened.
    pub ts: OffsetDateTime,
    /// What happened.
    pub kind: EventKind,
}

impl Event {
    /// An event that happened now.
    pub fn now(session_id: SessionId, kind: EventKind) -> Self {
        Self {
            session_id,
            ts: OffsetDateTime::now_utc(),
            kind,
        }
    }
}

impl Event {
    /// A short, one-line description for activity feeds and replays.
    /// Events that are bookkeeping rather than activity return `None`.
    pub fn describe(&self) -> Option<String> {
        let text = match &self.kind {
            EventKind::State(state) => format!("state: {state}"),
            EventKind::ToolUse { tool, summary } => match summary {
                Some(summary) => format!("{tool} {summary}"),
                None => tool.clone(),
            },
            EventKind::FileEdit { path } => format!("edit {path}"),
            EventKind::Command { command } => format!("$ {command}"),
            EventKind::TestRun { command, passed } => {
                let outcome = match passed {
                    Some(true) => "passed",
                    Some(false) => "FAILED",
                    None => "ran",
                };
                format!("tests {outcome}: {command}")
            }
            EventKind::Message { text } => text.clone(),
            EventKind::Session { .. } | EventKind::Trust { .. } => return None,
        };
        // Keep feeds to one line.
        Some(text.split_whitespace().collect::<Vec<_>>().join(" "))
    }
}

/// `5s`, `3m`, `2h` or `4d` for an age in seconds.
pub fn format_age(seconds: i64) -> String {
    let seconds = seconds.max(0);
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3599 => format!("{}m", seconds / 60),
        3600..=86_399 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86_400),
    }
}

/// Whether `command` runs a test suite, judged by well-known test runners.
pub fn is_test_command(command: &str) -> bool {
    const RUNNERS: &[&str] = &[
        "cargo test",
        "cargo nextest",
        "pytest",
        "python -m pytest",
        "python3 -m pytest",
        "python -m unittest",
        "npm test",
        "npm run test",
        "yarn test",
        "pnpm test",
        "pnpm run test",
        "bun test",
        "npx jest",
        "npx vitest",
        "jest",
        "vitest",
        "go test",
        "mvn test",
        "gradle test",
        "gradlew test",
        "dotnet test",
        "rspec",
        "bundle exec rspec",
        "phpunit",
        "make test",
        "ctest",
        "deno test",
    ];
    // Any segment of a `&&` / `;` / `|` chain may be the test run.
    command
        .split(['&', ';', '|', '\n'])
        .map(strip_env_prefix)
        .any(|segment| {
            RUNNERS.iter().any(|runner| {
                segment == *runner
                    || segment.starts_with(&format!("{runner} "))
                    || segment.contains(&format!("/{runner} "))
            })
        })
}

/// Trim `segment` and drop leading `NAME=value` environment assignments.
fn strip_env_prefix(segment: &str) -> &str {
    let mut rest = segment.trim();
    while let Some((first, tail)) = rest.split_once(char::is_whitespace) {
        let is_assignment = first.contains('=') && !first.starts_with('=') && !first.contains('/');
        if !is_assignment {
            break;
        }
        rest = tail.trim_start();
    }
    rest
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_names_match_the_protocol() {
        let k = EventKind::State(SessionState::Idle);
        assert_eq!(k.type_name(), "state");
        let k = EventKind::TestRun {
            command: "x".into(),
            passed: None,
        };
        assert_eq!(k.type_name(), "test_run");
    }

    #[test]
    fn describes_activity_on_one_line() {
        let d = |kind| Event::now(SessionId(1), kind).describe();
        assert_eq!(
            d(EventKind::ToolUse {
                tool: "Edit".into(),
                summary: Some("a.rs".into())
            }),
            Some("Edit a.rs".into())
        );
        assert_eq!(
            d(EventKind::Command {
                command: "ls
  -la"
                .into()
            }),
            Some("$ ls -la".into())
        );
        assert_eq!(
            d(EventKind::TestRun {
                command: "cargo test".into(),
                passed: Some(false)
            }),
            Some("tests FAILED: cargo test".into())
        );
        assert_eq!(
            d(EventKind::State(SessionState::Waiting)),
            Some("state: waiting".into())
        );
    }

    #[test]
    fn bookkeeping_events_have_no_description() {
        let session = EventKind::Session {
            name: "n".into(),
            agent: "a".into(),
            cwd: "c".into(),
        };
        assert_eq!(Event::now(SessionId(1), session).describe(), None);
    }

    #[test]
    fn ages_pick_a_sensible_unit() {
        assert_eq!(format_age(-5), "0s");
        assert_eq!(format_age(59), "59s");
        assert_eq!(format_age(60), "1m");
        assert_eq!(format_age(3599), "59m");
        assert_eq!(format_age(7200), "2h");
        assert_eq!(format_age(200_000), "2d");
    }

    #[test]
    fn detects_test_commands() {
        for cmd in [
            "cargo test",
            "cargo test -p sakshi-core",
            "pytest -q tests/",
            "cd app && npm test",
            "npm run test -- --watch=false",
            "go test ./...",
            "FOO=1 make test",
        ] {
            assert!(is_test_command(cmd), "{cmd}");
        }
    }

    #[test]
    fn ignores_non_test_commands() {
        for cmd in [
            "cargo build",
            "ls -la",
            "echo cargo test",
            "git commit -m test",
        ] {
            assert!(!is_test_command(cmd), "{cmd}");
        }
    }
}
