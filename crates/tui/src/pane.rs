//! One session's runtime state: its PTY, its emulated screen, and what the
//! trust engine knows about it.

use std::io;
use std::sync::Arc;

use ratatui::crossterm::event::KeyEvent;
use sakshi_core::{evaluate, Diff, Event, EventKind, Session, SessionState, TrustReport};
use sakshi_git::Baseline;
use sakshi_pty::{ExitInfo, PtyError, PtySession, TermSize};

use crate::emulator::{TerminalEmulator, Vt100Emulator};
use crate::keys;

/// Events kept per session for the trust engine and the activity feed. When
/// the cap is hit the oldest tenth is dropped.
const MAX_EVENTS: usize = 5000;

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

/// Something that can say what a session changed. A trait so the workspace
/// can be tested without running git.
pub(crate) trait DiffSource: Send + Sync {
    fn diff(&self) -> io::Result<Diff>;
}

impl DiffSource for Baseline {
    fn diff(&self) -> io::Result<Diff> {
        Baseline::diff(self)
    }
}

/// Whether the session's repository snapshot is available.
#[derive(Clone)]
pub(crate) enum GitState {
    /// Still being captured.
    Pending,
    /// The working directory is not in a git repository.
    NotARepo,
    /// Ready: diffs can be computed against this snapshot.
    Ready(Arc<dyn DiffSource>),
}

/// A running (or ended) session with its terminal.
pub(crate) struct Pane {
    pub session: Session,
    /// How the child ended, once it has.
    pub exit: Option<ExitInfo>,
    /// Latest message the agent reported, used as notification text.
    pub last_message: Option<String>,
    /// What the session has done, oldest first.
    pub events: Vec<Event>,
    /// Latest diff against the session's baseline, if one was computed.
    pub diff: Option<Diff>,
    /// The current trust verdict.
    pub trust: TrustReport,
    pub git: GitState,
    /// A diff is being computed.
    pub diff_in_flight: bool,
    /// Another diff was asked for while one was running.
    pub diff_pending: bool,
    /// Events or the diff changed since the verdict was computed.
    trust_stale: bool,
    pty: Box<dyn Pty>,
    emulator: Box<dyn TerminalEmulator>,
}

impl Pane {
    pub fn new(session: Session, pty: Box<dyn Pty>, size: TermSize) -> Self {
        Self {
            session,
            exit: None,
            last_message: None,
            events: Vec::new(),
            diff: None,
            trust: TrustReport::clean(),
            git: GitState::Pending,
            diff_in_flight: false,
            diff_pending: false,
            trust_stale: false,
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

    /// Record an agent event and apply it to the session. Returns the new
    /// state if the state changed.
    pub fn apply(&mut self, event: &Event) -> Option<SessionState> {
        if event.session_id != self.session.id {
            return None;
        }
        self.record(event);
        if let EventKind::Message { text } = &event.kind {
            self.last_message = Some(text.clone());
        }
        self.session.apply(event).then_some(self.session.state)
    }

    fn record(&mut self, event: &Event) {
        if self.events.len() >= MAX_EVENTS {
            self.events.drain(..MAX_EVENTS / 10);
        }
        self.events.push(event.clone());
        self.trust_stale = true;
    }

    /// Store a freshly computed diff.
    pub fn set_diff(&mut self, diff: Diff) {
        self.diff = Some(diff);
        self.trust_stale = true;
    }

    /// Recompute the trust verdict if anything changed since the last time.
    /// Returns the new report only when it differs from the old one.
    pub fn refresh_trust(&mut self) -> Option<&TrustReport> {
        if !self.trust_stale {
            return None;
        }
        self.trust_stale = false;
        let report = evaluate(&self.events, self.diff.as_ref());
        if report == self.trust {
            None
        } else {
            self.trust = report;
            Some(&self.trust)
        }
    }

    /// Kill the child. Errors are ignored: it may already be gone.
    pub fn kill(&mut self) {
        let _ = self.pty.kill();
    }
}
