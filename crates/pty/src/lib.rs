//! PTY spawning, resizing and I/O.
//!
//! Wraps `portable-pty` (Unix PTY and Windows ConPTY). Output is delivered on a
//! channel so callers can `select!` over it; no terminal emulation happens here.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use std::io::{Read, Write};
use std::path::PathBuf;
use std::thread;

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use tokio::sync::mpsc;

/// Errors from PTY operations.
#[derive(Debug, thiserror::Error)]
pub enum PtyError {
    /// The OS refused to open or configure the PTY, or to spawn the program.
    #[error("pty error: {0}")]
    Pty(String),
    /// Reading from or writing to the PTY failed.
    #[error("pty i/o error: {0}")]
    Io(#[from] std::io::Error),
}

fn pty_err(e: impl std::fmt::Display) -> PtyError {
    PtyError::Pty(e.to_string())
}

/// Size of the PTY in character cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TermSize {
    /// Number of rows.
    pub rows: u16,
    /// Number of columns.
    pub cols: u16,
}

impl From<TermSize> for PtySize {
    fn from(s: TermSize) -> Self {
        PtySize {
            rows: s.rows,
            cols: s.cols,
            pixel_width: 0,
            pixel_height: 0,
        }
    }
}

/// What to run and where.
#[derive(Debug, Clone)]
pub struct SpawnConfig {
    /// Program to execute.
    pub program: String,
    /// Arguments passed to the program.
    pub args: Vec<String>,
    /// Working directory; the current directory when `None`.
    pub cwd: Option<PathBuf>,
    /// Extra environment variables for the child, on top of the inherited ones.
    pub env: Vec<(String, String)>,
    /// Initial size.
    pub size: TermSize,
}

/// How a child process ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitInfo {
    /// Whether the exit code was zero.
    pub success: bool,
    /// Raw exit code.
    pub code: u32,
}

/// Raw output chunks read from the PTY.
pub type OutputReceiver = mpsc::UnboundedReceiver<Vec<u8>>;

/// A running child process attached to a PTY.
pub struct PtySession {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
}

impl PtySession {
    /// Spawn `config.program` in a new PTY.
    ///
    /// Returns the session and a receiver of raw output chunks. The receiver
    /// closes when the child's output ends (normally: the child exited).
    pub fn spawn(config: SpawnConfig) -> Result<(Self, OutputReceiver), PtyError> {
        let pair = native_pty_system()
            .openpty(config.size.into())
            .map_err(pty_err)?;

        let mut cmd = CommandBuilder::new(&config.program);
        cmd.args(&config.args);
        if let Some(cwd) = &config.cwd {
            cmd.cwd(cwd);
        }
        for (key, value) in &config.env {
            cmd.env(key, value);
        }
        let child = pair.slave.spawn_command(cmd).map_err(pty_err)?;
        // Close our copy of the slave so reads see EOF when the child exits.
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().map_err(pty_err)?;
        let writer = pair.master.take_writer().map_err(pty_err)?;

        let (tx, rx) = mpsc::unbounded_channel();
        thread::Builder::new()
            .name("sakshi-pty-reader".into())
            .spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if tx.send(buf[..n].to_vec()).is_err() {
                                break;
                            }
                        }
                    }
                }
            })?;

        Ok((
            Self {
                master: pair.master,
                writer,
                child,
            },
            rx,
        ))
    }

    /// Send input bytes to the child.
    pub fn write(&mut self, bytes: &[u8]) -> Result<(), PtyError> {
        self.writer.write_all(bytes)?;
        self.writer.flush()?;
        Ok(())
    }

    /// Resize the PTY.
    pub fn resize(&self, size: TermSize) -> Result<(), PtyError> {
        self.master.resize(size.into()).map_err(pty_err)
    }

    /// Exit status if the child has exited, `None` while it runs.
    pub fn try_wait(&mut self) -> Result<Option<ExitInfo>, PtyError> {
        let status = self.child.try_wait()?;
        Ok(status.map(|s| ExitInfo {
            success: s.success(),
            code: s.exit_code(),
        }))
    }

    /// Kill the child. Succeeds if it already exited.
    pub fn kill(&mut self) -> Result<(), PtyError> {
        if self.try_wait()?.is_some() {
            return Ok(());
        }
        self.child.kill()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn echo_config() -> SpawnConfig {
        #[cfg(windows)]
        let (program, args) = (
            "cmd.exe",
            vec!["/C".to_string(), "echo sakshi-ok".to_string()],
        );
        #[cfg(not(windows))]
        let (program, args) = ("sh", vec!["-c".to_string(), "echo sakshi-ok".to_string()]);
        SpawnConfig {
            program: program.into(),
            args,
            cwd: None,
            env: Vec::new(),
            size: TermSize { rows: 24, cols: 80 },
        }
    }

    #[tokio::test]
    async fn spawn_captures_child_output_and_exit() {
        let (mut session, mut rx) = PtySession::spawn(echo_config()).expect("spawn");
        let mut out = Vec::new();
        let mut answered_dsr = false;
        let collect = async {
            while let Some(chunk) = rx.recv().await {
                out.extend_from_slice(&chunk);
                // Act as the host terminal: ConPTY holds the child until its
                // cursor-position query is answered.
                if !answered_dsr && out.windows(4).any(|w| w == b"\x1b[6n") {
                    session.write(b"\x1b[1;1R").expect("answer dsr");
                    answered_dsr = true;
                }
                if String::from_utf8_lossy(&out).contains("sakshi-ok") {
                    break;
                }
            }
        };
        tokio::time::timeout(Duration::from_secs(10), collect)
            .await
            .expect("timed out waiting for output");
        assert!(String::from_utf8_lossy(&out).contains("sakshi-ok"));

        // The child exits on its own; poll briefly for its status.
        let mut exit = None;
        for _ in 0..100 {
            exit = session.try_wait().expect("try_wait");
            if exit.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(exit.map(|e| e.success), Some(true));
    }

    #[tokio::test]
    async fn spawn_passes_environment_to_the_child() {
        #[cfg(windows)]
        let (program, args) = (
            "cmd.exe",
            vec!["/C".to_string(), "echo %SAKSHI_PROBE%".to_string()],
        );
        #[cfg(not(windows))]
        let (program, args) = (
            "sh",
            vec!["-c".to_string(), "echo $SAKSHI_PROBE".to_string()],
        );
        let cfg = SpawnConfig {
            program: program.into(),
            args,
            cwd: None,
            env: vec![("SAKSHI_PROBE".into(), "env-ok-42".into())],
            size: TermSize { rows: 24, cols: 80 },
        };
        let (mut session, mut rx) = PtySession::spawn(cfg).expect("spawn");
        let mut out = Vec::new();
        let mut answered = false;
        let collect = async {
            while let Some(chunk) = rx.recv().await {
                out.extend_from_slice(&chunk);
                if !answered && out.windows(4).any(|w| w == b"\x1b[6n") {
                    session.write(b"\x1b[1;1R").expect("answer dsr");
                    answered = true;
                }
                if String::from_utf8_lossy(&out).contains("env-ok-42") {
                    break;
                }
            }
        };
        tokio::time::timeout(Duration::from_secs(10), collect)
            .await
            .expect("timed out waiting for output");
        assert!(String::from_utf8_lossy(&out).contains("env-ok-42"));
    }

    #[test]
    fn spawn_missing_program_fails() {
        let mut cfg = echo_config();
        cfg.program = "sakshi-definitely-not-a-program".into();
        assert!(PtySession::spawn(cfg).is_err());
    }
}
