//! Sakshi core: the session and event model, the event bus and the trust
//! engine, shared by every other crate.
//!
//! This crate has no UI and no OS-specific code.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod bus;
mod diff;
mod event;
mod session;
mod trust;

pub use bus::EventBus;
pub use diff::{parse_unified_diff, ChangeKind, Diff, FileChange};
pub use event::{format_age, is_test_command, Event, EventKind};
pub use session::{
    AgentKind, ParseSessionIdError, ParseStateError, Session, SessionId, SessionState,
};
pub use trust::{evaluate, is_test_path, Badge, Finding, ParseTrustError, Severity, TrustReport};
