//! Sakshi core: the session and event model shared by every other crate.
//!
//! This crate has no UI and no OS-specific code.

mod event;
mod session;

pub use event::{is_test_command, Event, EventKind};
pub use session::{
    AgentKind, ParseSessionIdError, ParseStateError, Session, SessionId, SessionState,
};
