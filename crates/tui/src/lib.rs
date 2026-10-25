//! Sakshi's terminal UI: a session list next to the attached session's terminal.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod app;
pub mod emulator;
mod keys;
mod layout;
pub mod notify;
mod pane;
mod ui;
mod workspace;

pub use app::{run, RunConfig, TuiError};
