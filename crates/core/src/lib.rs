//! Sakshi core: the session and event model, the event bus and the trust
//! engine, shared by every other crate.
//!
//! This crate has no UI and no OS-specific code.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod bus;
mod diff;
mod event;
mod redact;
mod report;
mod session;
mod summary;
mod trust;

pub use bus::EventBus;
pub use diff::{parse_unified_diff, ChangeKind, Diff, FileChange};
pub use event::{format_age, is_test_command, Event, EventKind};
pub use redact::{redact_event, redact_secrets, REDACTED};
pub use report::{exit_summary, timeline_line, Report, SessionVerdict};
pub use session::{
    AgentKind, ParseSessionIdError, ParseStateError, Session, SessionId, SessionState,
};
pub use summary::{summarize, Summary};
pub use trust::{
    edited_path, evaluate, evaluate_with, glob_match, hooks_silent, is_test_path, shared_edits,
    Badge, Concern, Finding, Observations, ParseTrustError, PathRule, Severity, SharedEdit,
    TrustConfig, TrustReport, HOOK_SILENCE_OUTPUT_BYTES, HOOK_SILENCE_SECS,
};
