//! Diagnostics go to a log file: the terminal belongs to the TUI.

use std::fs::OpenOptions;
use std::sync::Mutex;

use tracing_subscriber::EnvFilter;

/// Start logging to `sakshi.log` in the user's data directory.
///
/// Best effort: if the file cannot be opened, logging stays off. The level
/// comes from `SAKSHI_LOG` (default `warn`).
pub fn init() {
    let Some(dir) = sakshi_store::data_dir() else {
        return;
    };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let Ok(file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("sakshi.log"))
    else {
        return;
    };
    let filter = EnvFilter::try_from_env("SAKSHI_LOG").unwrap_or_else(|_| EnvFilter::new("warn"));
    // A second init (e.g. in tests) fails harmlessly.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(Mutex::new(file))
        .with_ansi(false)
        .try_init();
}
