use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::event::{Event, EventKind};

/// Opaque identifier of a session. Rendered as 16 lowercase hex digits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(pub u64);

impl SessionId {
    /// A fresh id, unique across processes for all practical purposes.
    pub fn generate() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let count = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut x = nanos
            ^ (u64::from(std::process::id()) << 32)
            ^ count.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        // splitmix64 finalizer: spreads similar inputs across all bits.
        x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        SessionId(x ^ (x >> 31))
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

/// A string was not a valid [`SessionId`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid session id: {0:?}")]
pub struct ParseSessionIdError(String);

impl FromStr for SessionId {
    type Err = ParseSessionIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() || s.len() > 16 {
            return Err(ParseSessionIdError(s.to_string()));
        }
        u64::from_str_radix(s, 16)
            .map(SessionId)
            .map_err(|_| ParseSessionIdError(s.to_string()))
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
    /// Started, nothing in progress yet.
    Idle,
    /// Actively working.
    Working,
    /// Needs the user: a question, a permission prompt, or a finished turn.
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

    /// The protocol name of this state.
    pub fn as_str(self) -> &'static str {
        match self {
            SessionState::Idle => "idle",
            SessionState::Working => "working",
            SessionState::Waiting => "waiting",
            SessionState::Done => "done",
            SessionState::Error => "error",
        }
    }

    /// Whether the session has ended; no further state changes are possible.
    pub fn is_terminal(self) -> bool {
        matches!(self, SessionState::Done | SessionState::Error)
    }
}

impl fmt::Display for SessionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A string was not a valid [`SessionState`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown session state: {0:?}")]
pub struct ParseStateError(String);

impl FromStr for SessionState {
    type Err = ParseStateError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "idle" => Ok(SessionState::Idle),
            "working" => Ok(SessionState::Working),
            "waiting" => Ok(SessionState::Waiting),
            "done" => Ok(SessionState::Done),
            "error" => Ok(SessionState::Error),
            other => Err(ParseStateError(other.to_string())),
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

    /// Update the session from an event. Events for other sessions are ignored,
    /// and a session that has ended stays ended.
    ///
    /// Returns `true` if the state changed.
    pub fn apply(&mut self, event: &Event) -> bool {
        if event.session_id != self.id {
            return false;
        }
        match event.kind {
            EventKind::State(next) if !self.state.is_terminal() && next != self.state => {
                self.state = next;
                true
            }
            _ => false,
        }
    }

    /// Mark the session as ended. A session that already ended is unchanged.
    pub fn finish(&mut self, success: bool) {
        if !self.state.is_terminal() {
            self.state = if success {
                SessionState::Done
            } else {
                SessionState::Error
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        Session::new(
            SessionId(1),
            "shell",
            AgentKind::Generic,
            PathBuf::from("."),
        )
    }

    fn state_event(id: u64, state: SessionState) -> Event {
        Event::now(SessionId(id), EventKind::State(state))
    }

    #[test]
    fn new_session_starts_idle() {
        let s = session();
        assert_eq!(s.state, SessionState::Idle);
        assert_eq!(s.name, "shell");
    }

    #[test]
    fn every_state_has_a_distinct_icon_and_name() {
        let states = [
            SessionState::Idle,
            SessionState::Working,
            SessionState::Waiting,
            SessionState::Done,
            SessionState::Error,
        ];
        for (i, a) in states.iter().enumerate() {
            assert_eq!(a.as_str().parse::<SessionState>().as_ref(), Ok(a));
            for b in &states[i + 1..] {
                assert_ne!(a.icon(), b.icon());
                assert_ne!(a.as_str(), b.as_str());
            }
        }
    }

    #[test]
    fn unknown_state_is_rejected() {
        assert!("sleeping".parse::<SessionState>().is_err());
    }

    #[test]
    fn session_id_round_trips_through_text() {
        let id = SessionId::generate();
        let text = id.to_string();
        assert_eq!(text.len(), 16);
        assert_eq!(text.parse::<SessionId>(), Ok(id));
        assert_eq!("ff".parse::<SessionId>(), Ok(SessionId(255)));
    }

    #[test]
    fn bad_session_ids_are_rejected() {
        for s in ["", "xyz", "00000000000000001", "-1", "0x1"] {
            assert!(s.parse::<SessionId>().is_err(), "{s:?}");
        }
    }

    #[test]
    fn generated_ids_differ() {
        assert_ne!(SessionId::generate(), SessionId::generate());
    }

    #[test]
    fn apply_updates_state_for_own_events_only() {
        let mut s = session();
        assert!(s.apply(&state_event(1, SessionState::Working)));
        assert_eq!(s.state, SessionState::Working);
        assert!(!s.apply(&state_event(2, SessionState::Waiting)));
        assert_eq!(s.state, SessionState::Working);
        assert!(!s.apply(&state_event(1, SessionState::Working)));
    }

    #[test]
    fn apply_ignores_non_state_events() {
        let mut s = session();
        let ev = Event::now(SessionId(1), EventKind::Message { text: "hi".into() });
        assert!(!s.apply(&ev));
    }

    #[test]
    fn ended_session_stays_ended() {
        let mut s = session();
        s.finish(true);
        assert_eq!(s.state, SessionState::Done);
        assert!(!s.apply(&state_event(1, SessionState::Working)));
        s.finish(false);
        assert_eq!(s.state, SessionState::Done);
    }
}
