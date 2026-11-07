//! One session's runtime state: its PTY, its emulated screen, and what the
//! trust engine knows about it.

use std::collections::BTreeSet;
use std::io;
use std::sync::Arc;

use ratatui::crossterm::event::KeyEvent;
use sakshi_core::{
    edited_path, evaluate_with, hooks_silent, summarize, AgentKind, Diff, Event, EventKind,
    Observations, Session, SessionState, SharedEdit, Summary, TrustConfig, TrustReport,
};
use sakshi_git::Baseline;
use sakshi_pty::{ExitInfo, PtyError, PtySession, TermSize};
use time::OffsetDateTime;

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
    /// When the user last looked at this session; what happened after is
    /// what they missed.
    pub last_seen: OffsetDateTime,
    pub git: GitState,
    /// A diff is being computed.
    pub diff_in_flight: bool,
    /// Another diff was asked for while one was running.
    pub diff_pending: bool,
    /// Events or the diff changed since the verdict was computed.
    trust_stale: bool,
    /// The user's adjustments to the trust rules.
    trust_config: Arc<TrustConfig>,
    /// Bytes the child has printed, for spotting hooks that never report.
    output_bytes: u64,
    /// Whether the last verdict included silent hooks.
    hooks_were_silent: bool,
    /// Absolute paths this session has edited, for spotting collisions.
    pub edited: BTreeSet<String>,
    /// Files other running sessions have edited too.
    shared_edits: Vec<SharedEdit>,
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
            last_seen: OffsetDateTime::now_utc(),
            git: GitState::Pending,
            diff_in_flight: false,
            diff_pending: false,
            trust_stale: false,
            trust_config: Arc::new(TrustConfig::default()),
            output_bytes: 0,
            hooks_were_silent: false,
            edited: BTreeSet::new(),
            shared_edits: Vec::new(),
            pty,
            emulator: Box::new(Vt100Emulator::new(size)),
        }
    }

    /// Use the user's trust configuration instead of the defaults.
    pub fn with_trust_config(mut self, config: Arc<TrustConfig>) -> Self {
        self.trust_config = config;
        self
    }

    pub fn emulator(&self) -> &dyn TerminalEmulator {
        self.emulator.as_ref()
    }

    /// Feed child output to the emulator and send back any answers it owes.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.output_bytes = self.output_bytes.saturating_add(bytes.len() as u64);
        self.emulator.process(bytes);
        let replies = self.emulator.take_replies();
        if !replies.is_empty() {
            // A failed write means the child is gone; `poll_exit` reports that.
            let _ = self.pty.write(&replies);
        }
    }

    /// Send a key press to the child.
    pub fn send_key(&mut self, key: KeyEvent) {
        self.snap_to_bottom();
        if let Some(bytes) = keys::encode_key(key, self.emulator.application_cursor()) {
            // A failed write means the child is gone; `poll_exit` reports that.
            let _ = self.pty.write(&bytes);
        }
    }

    /// Paste text into the child, wrapped the way it asked for.
    pub fn paste(&mut self, text: &str) {
        let bytes = keys::paste_bytes(text, self.emulator.bracketed_paste());
        // A failed write means the child is gone; `poll_exit` reports that.
        let _ = self.pty.write(&bytes);
        self.snap_to_bottom();
    }

    /// Rows the view is scrolled back from the live screen.
    pub fn scrolled(&self) -> usize {
        self.emulator.scrollback()
    }

    /// Scroll the view by `rows` (positive is back into history).
    pub fn scroll(&mut self, rows: isize) {
        let now = self.emulator.scrollback();
        self.emulator
            .set_scrollback(now.saturating_add_signed(rows));
    }

    /// Return to the live screen.
    pub fn snap_to_bottom(&mut self) {
        if self.emulator.scrollback() > 0 {
            self.emulator.set_scrollback(0);
        }
    }

    /// How many rows a "page" of scrolling is.
    pub fn page_rows(&self) -> isize {
        let rows = isize::try_from(self.emulator.size().rows).unwrap_or(isize::MAX);
        (rows - 1).max(1)
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
        if let EventKind::FileEdit { path } = &event.kind {
            self.edited.extend(edited_path(path));
        }
        self.events.push(event.clone());
        self.trust_stale = true;
    }

    /// Record which of this session's edited files other running sessions
    /// edited too; the verdict is recomputed if that changed.
    pub fn set_shared_edits(&mut self, shared: Vec<SharedEdit>) {
        if shared != self.shared_edits {
            self.shared_edits = shared;
            self.trust_stale = true;
        }
    }

    /// Store a freshly computed diff.
    pub fn set_diff(&mut self, diff: Diff) {
        self.diff = Some(diff);
        self.trust_stale = true;
    }

    /// Recompute the trust verdict if anything changed since the last time.
    /// Returns the new report only when it differs from the old one.
    pub fn refresh_trust(&mut self) -> Option<&TrustReport> {
        let observed = self.observations();
        // Silence depends on time and output, not just on new events.
        let silent = hooks_silent(&self.events, &observed);
        if silent != self.hooks_were_silent {
            self.hooks_were_silent = silent;
            self.trust_stale = true;
        }
        if !self.trust_stale {
            return None;
        }
        self.trust_stale = false;
        let report = evaluate_with(
            &self.events,
            self.diff.as_ref(),
            &self.trust_config,
            &observed,
        );
        if report == self.trust {
            None
        } else {
            self.trust = report;
            Some(&self.trust)
        }
    }

    /// What Sakshi has seen of the session beyond its events.
    fn observations(&self) -> Observations {
        let running_secs = self.session.started_at.elapsed().map_or(0, |d| d.as_secs());
        Observations {
            // Every agent with an adapter reports through hooks. Once the process
            // has ended there is nothing left to report.
            hooks_expected: self.session.kind != AgentKind::Generic && self.exit.is_none(),
            output_bytes: self.output_bytes,
            running_secs,
            root: Some(self.session.working_dir.display().to_string()),
            shared_edits: self.shared_edits.clone(),
        }
    }

    /// What happened since the user last looked.
    pub fn away_summary(&self) -> Summary {
        summarize(&self.events, Some(self.last_seen))
    }

    /// Kill the child. Errors are ignored: it may already be gone.
    pub fn kill(&mut self) {
        let _ = self.pty.kill();
    }
}
