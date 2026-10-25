//! End-to-end test of the real TUI: run `sakshi` inside a PTY, type at it, and
//! read its screen back through a terminal emulator.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::num::NonZeroU16;
use std::time::{Duration, Instant};

use sakshi_pty::{OutputReceiver, PtySession, SpawnConfig, TermSize};

const ROWS: u16 = 30;
const COLS: u16 = 110;
const DEADLINE: Duration = Duration::from_secs(40);
/// Ctrl+] as a terminal sends it.
const CTRL_RIGHT_BRACKET: u8 = 0x1d;

#[cfg(windows)]
const SHELL: &str = "cmd.exe";
#[cfg(not(windows))]
const SHELL: &str = "sh";

/// A running `sakshi` and the screen it has drawn so far.
struct Tui {
    pty: PtySession,
    rx: OutputReceiver,
    parser: vt100::Parser,
    raw: Vec<u8>,
    dsr_answered: usize,
}

impl Tui {
    fn start(args: &[&str]) -> Self {
        let (pty, rx) = PtySession::spawn(SpawnConfig {
            program: env!("CARGO_BIN_EXE_sakshi").to_string(),
            args: args.iter().map(|a| a.to_string()).collect(),
            cwd: None,
            env: Vec::new(),
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

    fn screen(&self) -> String {
        self.parser.screen().contents()
    }

    fn send(&mut self, bytes: &[u8]) {
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
    async fn wait_for(&mut self, what: &str, done: impl Fn(&str) -> bool) {
        let started = Instant::now();
        loop {
            if done(&self.screen()) {
                return;
            }
            assert!(
                started.elapsed() < DEADLINE,
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
    async fn wait_for_exit(&mut self) -> bool {
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

#[tokio::test]
async fn runs_a_shell_detaches_starts_a_second_session_and_quits() {
    let mut tui = Tui::start(&["--no-notify", SHELL]);

    // The first session starts attached, beside the session list.
    tui.wait_for("the session list", |s| {
        s.contains("Sessions") && s.contains("1 ")
    })
    .await;
    tui.wait_for("the attached footer", |s| s.contains("Ctrl+] detach"))
        .await;

    // Typing reaches the shell and its output comes back.
    tui.send(b"echo sakshi-e2e-ok\r");
    tui.wait_for("the echoed output", |s| {
        s.matches("sakshi-e2e-ok").count() >= 2
    })
    .await;

    // Ctrl+] detaches to the list.
    tui.send(&[CTRL_RIGHT_BRACKET]);
    tui.wait_for("the list footer", |s| s.contains("Enter attach"))
        .await;

    // `n` starts a second session and attaches to it.
    tui.send(b"n");
    tui.wait_for("a second session", |s| {
        s.contains("2 ") && s.contains("Ctrl+] detach")
    })
    .await;

    // Detach again; quitting with running sessions asks first.
    tui.send(&[CTRL_RIGHT_BRACKET]);
    tui.wait_for("the list footer again", |s| s.contains("Enter attach"))
        .await;
    tui.send(b"q");
    tui.wait_for("the quit confirmation", |s| s.contains("q again"))
        .await;
    tui.send(b"q");

    assert!(tui.wait_for_exit().await, "sakshi should exit cleanly");
}
