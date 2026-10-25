//! One session's runtime state: its PTY, its emulated screen, and its model.

use ratatui::crossterm::event::KeyEvent;
use sakshi_core::{Event, EventKind, Session, SessionState};
use sakshi_pty::{ExitInfo, PtyError, PtySession, TermSize};

use crate::emulator::{TerminalEmulator, Vt100Emulator};
use crate::keys;

/// The parts of a PTY a pane needs. A trait so panes can be tested without
/// spawning real processes.
pub(crate) trait Pty: Send {
    fn write(&mut self, bytes: &[u8]) -> Result<(), PtyError>;
    fn resize(&mut self, size: TermSize) -> Result<(), PtyError>;
    fn try_wait(&mut self) -> Result<Option<ExitInfo>, PtyError>;
    fn kill(&mut self) -> Result<(), PtyError>;
}

impl Pty for PtySession {
    fn write(&mut self, bytes: &[u8]) -> Result<(), PtyError> {
        PtySession::write(self, bytes)
    }

    fn resize(&mut self, size: TermSize) -> Result<(), PtyError> {
        PtySession::resize(self, size)
    }

    fn try_wait(&mut self) -> Result<Option<ExitInfo>, PtyError> {
        PtySession::try_wait(self)
    }

    fn kill(&mut self) -> Result<(), PtyError> {
        PtySession::kill(self)
    }
}

/// A running (or ended) session with its terminal.
pub(crate) struct Pane {
    pub session: Session,
    /// How the child ended, once it has.
    pub exit: Option<ExitInfo>,
    /// Latest message the agent reported, used as notification text.
    pub last_message: Option<String>,
    pty: Box<dyn Pty>,
    emulator: Box<dyn TerminalEmulator>,
}

impl Pane {
    pub fn new(session: Session, pty: Box<dyn Pty>, size: TermSize) -> Self {
        Self {
            session,
            exit: None,
            last_message: None,
            pty,
            emulator: Box::new(Vt100Emulator::new(size)),
        }
    }

    pub fn emulator(&self) -> &dyn TerminalEmulator {
        self.emulator.as_ref()
    }

    /// Feed child output to the emulator and send back any answers it owes.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.emulator.process(bytes);
        let replies = self.emulator.take_replies();
        if !replies.is_empty() {
            // A failed write means the child is gone; `poll_exit` reports that.
            let _ = self.pty.write(&replies);
        }
    }

    /// Send a key press to the child.
    pub fn send_key(&mut self, key: KeyEvent) {
        if let Some(bytes) = keys::encode_key(key, self.emulator.application_cursor()) {
            // A failed write means the child is gone; `poll_exit` reports that.
            let _ = self.pty.write(&bytes);
        }
    }

    pub fn resize(&mut self, size: TermSize) {
        self.emulator.resize(size);
        // A failed resize means the child is gone; `poll_exit` reports that.
        let _ = self.pty.resize(size);
    }

    /// Check whether the child has exited. Returns the new state if this call
    /// is the one that ended the session.
    pub fn poll_exit(&mut self) -> Result<Option<SessionState>, PtyError> {
        if self.exit.is_some() {
            return Ok(None);
        }
        let Some(info) = self.pty.try_wait()? else {
            return Ok(None);
        };
        let before = self.session.state;
        self.session.finish(info.success);
        self.exit = Some(info);
        Ok((self.session.state != before).then_some(self.session.state))
    }

    /// Apply an agent event. Returns the new state if the state changed.
    pub fn apply(&mut self, event: &Event) -> Option<SessionState> {
        if event.session_id != self.session.id {
            return None;
        }
        if let EventKind::Message { text } = &event.kind {
            self.last_message = Some(text.clone());
        }
        self.session.apply(event).then_some(self.session.state)
    }

    /// Kill the child. Errors are ignored: it may already be gone.
    pub fn kill(&mut self) {
        let _ = self.pty.kill();
    }
}
