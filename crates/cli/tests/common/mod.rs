//! Shared harness for end-to-end tests: run `sakshi` inside a PTY, type at it,
//! and read its screen back through a terminal emulator.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::num::NonZeroU16;
use std::path::Path;
use std::time::{Duration, Instant};

use sakshi_pty::{OutputReceiver, PtySession, SpawnConfig, TermSize};

pub const ROWS: u16 = 30;
pub const COLS: u16 = 110;
pub const DEADLINE: Duration = Duration::from_secs(40);
/// Ctrl+] as a terminal sends it.
pub const CTRL_RIGHT_BRACKET: u8 = 0x1d;

#[cfg(windows)]
pub const SHELL: &str = "cmd.exe";
#[cfg(not(windows))]
pub const SHELL: &str = "sh";

/// The binary under test: the one just built, or the one named by
/// `SAKSHI_TEST_BIN` (used to test a released build).
pub fn sakshi_bin() -> String {
    std::env::var("SAKSHI_TEST_BIN")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_sakshi").to_string())
}

/// A running `sakshi` and the screen it has drawn so far.
pub struct Tui {
    pty: PtySession,
    rx: OutputReceiver,
    parser: vt100::Parser,
    raw: Vec<u8>,
    dsr_answered: usize,
}

impl Tui {
    pub fn start(args: &[&str], data_dir: &Path) -> Self {
        Self::start_with_env(args, data_dir, Vec::new())
    }

    pub fn start_with_env(args: &[&str], data_dir: &Path, mut env: Vec<(String, String)>) -> Self {
        env.push((
            "SAKSHI_DATA_DIR".to_string(),
            data_dir.to_string_lossy().into_owned(),
        ));
        let (pty, rx) = PtySession::spawn(SpawnConfig {
            program: sakshi_bin(),
            args: args.iter().map(|a| a.to_string()).collect(),
            cwd: None,
            env,
            size: TermSize {
                rows: ROWS,
                cols: COLS,
            },
        })
        .unwrap();
        Self {
            pty,
            rx,
            parser: vt100::Parser::new(
                NonZeroU16::new(ROWS).unwrap(),
                NonZeroU16::new(COLS).unwrap(),
                0,
            ),
            raw: Vec::new(),
            dsr_answered: 0,
        }
    }

    pub fn screen(&self) -> String {
        self.parser.screen().contents()
    }

    /// Everything the program has written so far, escape sequences included.
    pub fn raw(&self) -> &[u8] {
        &self.raw
    }

    pub fn send(&mut self, bytes: &[u8]) {
        self.pty.write(bytes).unwrap();
    }

    /// Feed one chunk to the emulator, answering cursor-position queries the
    /// way a real terminal would (Windows ConPTY waits for the answer).
    fn absorb(&mut self, chunk: &[u8]) {
        self.raw.extend_from_slice(chunk);
        self.parser.process(chunk);
        let queries = self.raw.windows(4).filter(|w| *w == b"\x1b[6n").count();
        while self.dsr_answered < queries {
            let (row, col) = self.parser.screen().cursor_position();
            let reply = format!("\x1b[{};{}R", row + 1, col + 1);
            self.pty.write(reply.as_bytes()).unwrap();
            self.dsr_answered += 1;
        }
    }

    /// Read output until `done(screen)` holds, or fail with the screen dump.
    pub async fn wait_for(&mut self, what: &str, done: impl Fn(&str) -> bool) {
        self.wait_for_within(what, DEADLINE, done).await;
    }

    pub async fn wait_for_within(
        &mut self,
        what: &str,
        limit: Duration,
        done: impl Fn(&str) -> bool,
    ) {
        let started = Instant::now();
        loop {
            if done(&self.screen()) {
                return;
            }
            assert!(
                started.elapsed() < limit,
                "timed out waiting for {what}; screen was:\n{}",
                self.screen()
            );
            if let Ok(Some(chunk)) =
                tokio::time::timeout(Duration::from_millis(100), self.rx.recv()).await
            {
                self.absorb(&chunk);
            }
        }
    }

    /// Wait for the process to exit; returns whether it exited successfully.
    pub async fn wait_for_exit(&mut self) -> bool {
        let started = Instant::now();
        loop {
            if let Some(info) = self.pty.try_wait().unwrap() {
                return info.success;
            }
            assert!(
                started.elapsed() < DEADLINE,
                "sakshi did not exit; screen was:\n{}",
                self.screen()
            );
            // Keep draining so a full pipe cannot stall the child.
            if let Ok(Some(chunk)) =
                tokio::time::timeout(Duration::from_millis(100), self.rx.recv()).await
            {
                self.absorb(&chunk);
            }
        }
    }
}

/// A fresh scratch data directory, removed when dropped.
pub struct DataDir(pub std::path::PathBuf);

impl DataDir {
    pub fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("sakshi-e2e-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Self(dir)
    }
}

impl Drop for DataDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
