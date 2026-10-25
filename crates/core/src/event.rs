use time::OffsetDateTime;

use crate::session::{SessionId, SessionState};

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
