//! Sakshi core: the session model shared by every other crate.
//!
//! This crate has no UI and no OS-specific code.

mod session;

pub use session::{AgentKind, Session, SessionId, SessionState};
