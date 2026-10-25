//! Sakshi's terminal UI: renders one live PTY session next to a session list.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod app;
pub mod emulator;
mod keys;
mod layout;

pub use app::{run, RunConfig, TuiError};
