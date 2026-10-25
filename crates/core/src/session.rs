use std::fmt;
use std::path::PathBuf;
use std::time::SystemTime;

/// Opaque identifier of a session, unique within one Sakshi process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(pub u64);

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Which agent (if any) a session is running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentKind {
    /// Claude Code.
    Claude,
    /// A plain shell or any program Sakshi has no adapter for.
    Generic,
}

/// Lifecycle state of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    /// Running, nothing in progress.
    Idle,
    /// Actively working.
    Working,
    /// Blocked on the user.
    Waiting,
    /// Exited normally.
    Done,
    /// Exited with an error.
    Error,
}

impl SessionState {
    /// Single-character icon used in the session list.
    pub fn icon(self) -> char {
        match self {
            SessionState::Idle => '○',
            SessionState::Working => '●',
            SessionState::Waiting => '!',
            SessionState::Done => '✓',
            SessionState::Error => '✗',
        }
    }
}

/// One supervised terminal session.
#[derive(Debug, Clone)]
pub struct Session {
    /// Unique id.
    pub id: SessionId,
    /// Display name.
    pub name: String,
    /// Agent running in the session.
    pub kind: AgentKind,
    /// Working directory the session was started in.
    pub working_dir: PathBuf,
    /// Current state.
    pub state: SessionState,
    /// When the session started.
    pub started_at: SystemTime,
}

impl Session {
    /// Create a session in the [`SessionState::Idle`] state, started now.
    pub fn new(
        id: SessionId,
        name: impl Into<String>,
        kind: AgentKind,
        working_dir: PathBuf,
    ) -> Self {
        Self {
            id,
            name: name.into(),
            kind,
            working_dir,
            state: SessionState::Idle,
            started_at: SystemTime::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_session_starts_idle() {
        let s = Session::new(
            SessionId(1),
            "shell",
            AgentKind::Generic,
            PathBuf::from("."),
        );
        assert_eq!(s.state, SessionState::Idle);
        assert_eq!(s.name, "shell");
    }

    #[test]
    fn every_state_has_a_distinct_icon() {
        let icons = [
            SessionState::Idle,
            SessionState::Working,
            SessionState::Waiting,
            SessionState::Done,
            SessionState::Error,
        ]
        .map(SessionState::icon);
        for (i, a) in icons.iter().enumerate() {
            for b in &icons[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }
}
