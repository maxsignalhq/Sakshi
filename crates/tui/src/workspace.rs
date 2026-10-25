//! The set of sessions and what the user is doing with them.
//!
//! Pure state: no I/O happens here. Key presses and events come in; the
//! result says what the caller must do (quit, start a session, compute a
//! diff). Everything that happens is published on the event bus, which is how
//! the activity log hears about it.

use std::sync::Arc;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use sakshi_core::{
    format_age, Badge, Diff, Event, EventBus, EventKind, SessionId, SessionState, Severity,
};
use sakshi_pty::TermSize;
use time::OffsetDateTime;

use crate::layout;
use crate::notify::Notifier;
use crate::pane::{DiffSource, GitState, Pane};

/// Ticks (100 ms each) between diff refreshes of a working session.
const DIFF_REFRESH_TICKS: u64 = 100;

/// Shortest absence that earns a "while you were away" summary. Quick
/// switches between sessions should stay quiet.
const AWAY_MIN_SECONDS: i64 = 3;

/// A message laid over the terminal until a key is pressed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Overlay {
    pub title: String,
    pub lines: Vec<String>,
}

/// Where keyboard input goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Focus {
    /// Navigating the session list.
    List,
    /// Attached: keys go to the selected session's terminal.
    Terminal,
}

/// An agent that can be started from the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentInfo {
    /// Name shown in the key hints.
    pub name: String,
    /// Program to run.
    pub program: String,
    /// Key that starts it from the list.
    pub key: char,
}

/// What kind of session to start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NewSession {
    /// A shell.
    Shell,
    /// An agent, by program name.
    Agent(String),
}

/// What the caller must do after a key press.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Continue,
    Quit,
    Spawn(NewSession),
}

/// A destructive action waiting for a second key press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Confirm {
    Close(SessionId),
    Quit,
}

pub(crate) struct Workspace {
    pub panes: Vec<Pane>,
    pub selected: usize,
    pub focus: Focus,
    pub confirm: Option<Confirm>,
    /// One-off message for the footer, cleared by the next key press.
    pub notice: Option<String>,
    /// "While you were away" summary shown on re-attach.
    pub overlay: Option<Overlay>,
    pub agents: Vec<AgentInfo>,
    /// Whether the details panel is switched on.
    pub show_panel: bool,
    screen: Rect,
    notifier: Box<dyn Notifier>,
    bus: EventBus,
    diff_queue: Vec<SessionId>,
    ticks: u64,
}

/// Whether `key` is the detach chord, Ctrl+].
///
/// Unix terminals deliver 0x1D, which crossterm reports as Ctrl+5.
fn is_detach_chord(key: KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char(']') | KeyCode::Char('5'))
}

impl Workspace {
    pub fn new(
        screen: Rect,
        agents: Vec<AgentInfo>,
        notifier: Box<dyn Notifier>,
        bus: EventBus,
    ) -> Self {
        Self {
            panes: Vec::new(),
            selected: 0,
            focus: Focus::List,
            confirm: None,
            notice: None,
            overlay: None,
            agents,
            show_panel: false,
            screen,
            notifier,
            bus,
            diff_queue: Vec::new(),
            ticks: 0,
        }
    }

    /// The size every session's terminal should have right now.
    pub fn pty_size(&self) -> TermSize {
        layout::pty_size(self.screen, self.show_panel)
    }

    pub fn selected_pane(&self) -> Option<&Pane> {
        self.panes.get(self.selected)
    }

    /// Number of sessions whose process is still running.
    pub fn live_count(&self) -> usize {
        self.panes.iter().filter(|p| p.exit.is_none()).count()
    }

    /// Add a session. With `attach`, select it and send keys to it.
    pub fn add_pane(&mut self, pane: Pane, attach: bool) {
        self.bus.publish(&Event::now(
            pane.session.id,
            EventKind::Session {
                name: pane.session.name.clone(),
                agent: pane.session.kind.as_str().to_string(),
                cwd: pane.session.working_dir.display().to_string(),
            },
        ));
        self.panes.push(pane);
        if attach {
            self.selected = self.panes.len() - 1;
            self.focus = Focus::Terminal;
        }
    }

    /// A session name not used yet: `base`, then `base-2`, `base-3`, ...
    pub fn unique_name(&self, base: &str) -> String {
        let taken = |name: &str| self.panes.iter().any(|p| p.session.name == name);
        if !taken(base) {
            return base.to_string();
        }
        (2..)
            .map(|n| format!("{base}-{n}"))
            .find(|name| !taken(name))
            .unwrap_or_else(|| base.to_string())
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Outcome {
        self.notice = None;
        // Any key dismisses the summary; it is not passed on to the session.
        if self.overlay.take().is_some() {
            return Outcome::Continue;
        }
        match self.focus {
            Focus::Terminal => self.handle_terminal_key(key),
            Focus::List => self.handle_list_key(key),
        }
    }

    fn handle_terminal_key(&mut self, key: KeyEvent) -> Outcome {
        let Some(pane) = self.panes.get_mut(self.selected) else {
            self.focus = Focus::List;
            return Outcome::Continue;
        };
        // After the process ended there is nothing to type into.
        if is_detach_chord(key) || pane.exit.is_some() {
            self.focus = Focus::List;
            // Everything up to now has been seen.
            pane.last_seen = OffsetDateTime::now_utc();
            // Whatever the user just did in there is worth a look.
            let id = pane.session.id;
            self.request_diff(id);
        } else {
            pane.send_key(key);
        }
        Outcome::Continue
    }

    fn handle_list_key(&mut self, key: KeyEvent) -> Outcome {
        let pending = self.confirm.take();
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.select(-1),
            KeyCode::Down | KeyCode::Char('j') => self.select(1),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => self.attach(),
            KeyCode::Char(c @ '1'..='9') => {
                let index = usize::from(c as u8 - b'1');
                if index < self.panes.len() {
                    self.selected = index;
                    self.attach();
                }
            }
            KeyCode::Char('n') => return Outcome::Spawn(NewSession::Shell),
            KeyCode::Char('x') => self.close_selected(pending),
            KeyCode::Char('q') => return self.quit(pending),
            KeyCode::Char('t') | KeyCode::Tab => self.toggle_panel(),
            KeyCode::Char('r') => {
                if let Some(id) = self.selected_pane().map(|p| p.session.id) {
                    self.request_diff(id);
                }
            }
            KeyCode::Char(c) => {
                if let Some(agent) = self.agents.iter().find(|a| a.key == c) {
                    return Outcome::Spawn(NewSession::Agent(agent.program.clone()));
                }
            }
            _ => {}
        }
        Outcome::Continue
    }

    fn select(&mut self, delta: isize) {
        if self.panes.is_empty() {
            return;
        }
        let last = self.panes.len() - 1;
        self.selected = self.selected.saturating_add_signed(delta).min(last);
    }

    fn attach(&mut self) {
        let Some(pane) = self.panes.get(self.selected) else {
            return;
        };
        self.focus = Focus::Terminal;
        let away = (OffsetDateTime::now_utc() - pane.last_seen).whole_seconds();
        let summary = pane.away_summary();
        if away < AWAY_MIN_SECONDS || summary.is_quiet() {
            return;
        }
        let mut lines = summary.lines();
        let worst = pane
            .trust
            .findings
            .first()
            .filter(|f| f.severity != Severity::Info);
        if let Some(finding) = worst {
            lines.push(format!(
                "Trust: {} - {}",
                pane.trust.badge.as_str().to_uppercase(),
                finding.reason
            ));
        }
        self.overlay = Some(Overlay {
            title: format!(
                "{} - while you were away ({})",
                pane.session.name,
                format_age(away)
            ),
            lines,
        });
    }

    fn close_selected(&mut self, pending: Option<Confirm>) {
        let Some(pane) = self.panes.get_mut(self.selected) else {
            return;
        };
        let id = pane.session.id;
        if pane.exit.is_some() || pending == Some(Confirm::Close(id)) {
            pane.kill();
            self.panes.remove(self.selected);
            self.selected = self.selected.min(self.panes.len().saturating_sub(1));
        } else {
            self.confirm = Some(Confirm::Close(id));
        }
    }

    fn quit(&mut self, pending: Option<Confirm>) -> Outcome {
        if self.live_count() == 0 || pending == Some(Confirm::Quit) {
            Outcome::Quit
        } else {
            self.confirm = Some(Confirm::Quit);
            Outcome::Continue
        }
    }

    fn toggle_panel(&mut self) {
        self.show_panel = !self.show_panel;
        self.resize_panes();
    }

    fn resize_panes(&mut self) {
        let size = self.pty_size();
        for pane in &mut self.panes {
            pane.resize(size);
        }
    }

    /// Route child output to its pane.
    pub fn handle_output(&mut self, id: SessionId, bytes: &[u8]) {
        if let Some(pane) = self.panes.iter_mut().find(|p| p.session.id == id) {
            pane.feed(bytes);
        }
    }

    /// Apply an agent event to its session, publish it, and notify if the
    /// session now needs the user. Events for unknown sessions are ignored.
    pub fn handle_event(&mut self, event: &Event) {
        let Some(index) = self
            .panes
            .iter()
            .position(|p| p.session.id == event.session_id)
        else {
            return;
        };
        self.bus.publish(event);
        if let Some(state) = self.panes[index].apply(event) {
            self.on_state_change(index, state);
        }
    }

    fn on_state_change(&mut self, index: usize, state: SessionState) {
        self.notify_if_needed(index, state);
        // The agent paused or stopped: a good moment to see what it changed.
        if matches!(
            state,
            SessionState::Waiting | SessionState::Done | SessionState::Error
        ) {
            let id = self.panes[index].session.id;
            self.request_diff(id);
        }
    }

    /// Housekeeping for the 100 ms tick: detect exits, refresh stale trust
    /// verdicts, and re-diff sessions that are busy working.
    pub fn tick(&mut self) {
        self.poll_exits();
        self.ticks += 1;
        if self.ticks.is_multiple_of(DIFF_REFRESH_TICKS) {
            let working: Vec<SessionId> = self
                .panes
                .iter()
                .filter(|p| p.session.state == SessionState::Working)
                .map(|p| p.session.id)
                .collect();
            for id in working {
                self.request_diff(id);
            }
        }
        self.refresh_trust();
    }

    /// Detect processes that have exited since the last call.
    pub fn poll_exits(&mut self) {
        for index in 0..self.panes.len() {
            match self.panes[index].poll_exit() {
                Ok(Some(state)) => {
                    let id = self.panes[index].session.id;
                    self.bus.publish(&Event::now(id, EventKind::State(state)));
                    self.on_state_change(index, state);
                }
                Ok(None) => {}
                Err(e) => tracing::warn!("checking session exit failed: {e}"),
            }
        }
    }

    fn refresh_trust(&mut self) {
        for index in 0..self.panes.len() {
            let before = self.panes[index].trust.badge;
            let Some(report) = self.panes[index].refresh_trust().cloned() else {
                continue;
            };
            let id = self.panes[index].session.id;
            self.bus.publish(&Event::now(
                id,
                EventKind::Trust {
                    badge: report.badge,
                    findings: report.findings.clone(),
                },
            ));
            if report.badge == Badge::Red && before != Badge::Red && !self.is_attached(index) {
                let name = self.panes[index].session.name.clone();
                let reason = report
                    .findings
                    .first()
                    .map(|f| f.reason.clone())
                    .unwrap_or_default();
                self.notifier
                    .notify(&format!("{name}: trust alert"), &reason);
            }
        }
    }

    /// Ask for a fresh diff of a session. Duplicates are merged.
    fn request_diff(&mut self, id: SessionId) {
        if !self.diff_queue.contains(&id) {
            self.diff_queue.push(id);
        }
    }

    /// Diffs the caller should compute now. Sessions whose repository snapshot
    /// is not ready are skipped, and a session already being diffed is
    /// retried when its current diff lands.
    pub fn take_diff_requests(&mut self) -> Vec<(SessionId, Arc<dyn DiffSource>)> {
        let mut jobs = Vec::new();
        for id in std::mem::take(&mut self.diff_queue) {
            let Some(pane) = self.panes.iter_mut().find(|p| p.session.id == id) else {
                continue;
            };
            let GitState::Ready(baseline) = &pane.git else {
                continue;
            };
            if pane.diff_in_flight {
                pane.diff_pending = true;
            } else {
                pane.diff_in_flight = true;
                jobs.push((id, baseline.clone()));
            }
        }
        jobs
    }

    /// Record the outcome of capturing a session's repository snapshot.
    pub fn set_baseline(&mut self, id: SessionId, baseline: Option<Arc<dyn DiffSource>>) {
        if let Some(pane) = self.panes.iter_mut().find(|p| p.session.id == id) {
            pane.git = match baseline {
                Some(baseline) => GitState::Ready(baseline),
                None => GitState::NotARepo,
            };
        }
    }

    /// A requested diff finished. `None` means it failed.
    pub fn finish_diff(&mut self, id: SessionId, diff: Option<Diff>) {
        let Some(pane) = self.panes.iter_mut().find(|p| p.session.id == id) else {
            return;
        };
        pane.diff_in_flight = false;
        if let Some(diff) = diff {
            pane.set_diff(diff);
        }
        if std::mem::take(&mut pane.diff_pending) {
            self.request_diff(id);
        }
    }

    pub fn resize(&mut self, screen: Rect) {
        self.screen = screen;
        self.resize_panes();
    }

    /// Kill every running session. Used on shutdown.
    pub fn kill_all(&mut self) {
        for pane in &mut self.panes {
            pane.kill();
        }
    }

    fn is_attached(&self, index: usize) -> bool {
        self.focus == Focus::Terminal && self.selected == index
    }

    fn notify_if_needed(&mut self, index: usize, state: SessionState) {
        if self.is_attached(index) {
            return; // The user is looking at it.
        }
        let pane = &self.panes[index];
        let (headline, fallback) = match state {
            SessionState::Waiting => ("needs you", "Waiting for your input"),
            SessionState::Done => ("finished", "The session ended"),
            SessionState::Error => ("failed", "The session ended with an error"),
            SessionState::Idle | SessionState::Working => return,
        };
        let title = format!("{} {headline}", pane.session.name);
        let body = pane
            .last_message
            .clone()
            .unwrap_or_else(|| fallback.to_string());
        self.notifier.notify(&title, &body);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use ratatui::crossterm::event::KeyEventKind;
    use sakshi_core::{AgentKind, ChangeKind, FileChange, Session};
    use sakshi_pty::{ExitInfo, PtyError};
    use tokio::sync::mpsc::UnboundedReceiver;

    use super::*;
    use crate::pane::Pty;

    /// Shared record of what a fake PTY was asked to do.
    #[derive(Default)]
    pub struct FakeState {
        pub written: Vec<u8>,
        pub exit: Option<ExitInfo>,
        pub killed: bool,
        pub size: Option<TermSize>,
    }

    pub struct FakePty(pub Arc<Mutex<FakeState>>);

    impl Pty for FakePty {
        fn write(&mut self, bytes: &[u8]) -> Result<(), PtyError> {
            self.0.lock().unwrap().written.extend_from_slice(bytes);
            Ok(())
        }
        fn resize(&mut self, size: TermSize) -> Result<(), PtyError> {
            self.0.lock().unwrap().size = Some(size);
            Ok(())
        }
        fn try_wait(&mut self) -> Result<Option<ExitInfo>, PtyError> {
            Ok(self.0.lock().unwrap().exit)
        }
        fn kill(&mut self) -> Result<(), PtyError> {
            self.0.lock().unwrap().killed = true;
            Ok(())
        }
    }

    #[derive(Clone, Default)]
    pub struct Notes(pub Arc<Mutex<Vec<(String, String)>>>);

    impl Notifier for Notes {
        fn notify(&mut self, title: &str, body: &str) {
            self.0
                .lock()
                .unwrap()
                .push((title.to_string(), body.to_string()));
        }
    }

    const SCREEN: Rect = Rect {
        x: 0,
        y: 0,
        width: 160,
        height: 30,
    };

    pub fn fake_pane(id: u64, name: &str) -> (Pane, Arc<Mutex<FakeState>>) {
        let state = Arc::new(Mutex::new(FakeState::default()));
        let session = Session::new(SessionId(id), name, AgentKind::Generic, PathBuf::from("."));
        let size = layout::pty_size(SCREEN, false);
        (
            Pane::new(session, Box::new(FakePty(state.clone())), size),
            state,
        )
    }

    pub struct Fixture {
        pub ws: Workspace,
        pub states: Vec<Arc<Mutex<FakeState>>>,
        pub notes: Notes,
        pub log: UnboundedReceiver<Event>,
    }

    impl Fixture {
        /// Everything published on the bus so far.
        pub fn published(&mut self) -> Vec<Event> {
            let mut out = Vec::new();
            while let Ok(ev) = self.log.try_recv() {
                out.push(ev);
            }
            out
        }
    }

    pub fn fixture(names: &[&str]) -> Fixture {
        let notes = Notes::default();
        let agents = vec![AgentInfo {
            name: "Claude Code".into(),
            program: "claude".into(),
            key: 'c',
        }];
        let bus = EventBus::new();
        let log = bus.subscribe();
        let mut ws = Workspace::new(SCREEN, agents, Box::new(notes.clone()), bus);
        let mut states = Vec::new();
        for (i, name) in names.iter().enumerate() {
            let (pane, state) = fake_pane(i as u64 + 1, name);
            ws.add_pane(pane, false);
            states.push(state);
        }
        Fixture {
            ws,
            states,
            notes,
            log,
        }
    }

    pub fn workspace_with(names: &[&str]) -> (Workspace, Vec<Arc<Mutex<FakeState>>>, Notes) {
        let f = fixture(names);
        (f.ws, f.states, f.notes)
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new_with_kind(code, KeyModifiers::NONE, KeyEventKind::Press)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new_with_kind(KeyCode::Char(c), KeyModifiers::CONTROL, KeyEventKind::Press)
    }

    fn state_event(id: u64, state: SessionState) -> Event {
        Event::now(SessionId(id), EventKind::State(state))
    }

    fn exited(success: bool) -> Option<ExitInfo> {
        Some(ExitInfo {
            success,
            code: u32::from(!success),
        })
    }

    #[test]
    fn selection_moves_and_clamps() {
        let (mut ws, _, _) = workspace_with(&["a", "b", "c"]);
        assert_eq!(ws.selected, 0);
        ws.handle_key(press(KeyCode::Up));
        assert_eq!(ws.selected, 0);
        ws.handle_key(press(KeyCode::Char('j')));
        ws.handle_key(press(KeyCode::Down));
        assert_eq!(ws.selected, 2);
        ws.handle_key(press(KeyCode::Down));
        assert_eq!(ws.selected, 2);
        ws.handle_key(press(KeyCode::Char('k')));
        assert_eq!(ws.selected, 1);
    }

    #[test]
    fn digits_jump_to_a_session_and_attach() {
        let (mut ws, _, _) = workspace_with(&["a", "b", "c"]);
        ws.handle_key(press(KeyCode::Char('3')));
        assert_eq!((ws.selected, ws.focus), (2, Focus::Terminal));
        ws.handle_key(ctrl(']'));
        ws.handle_key(press(KeyCode::Char('9'))); // no such session
        assert_eq!((ws.selected, ws.focus), (2, Focus::List));
    }

    #[test]
    fn enter_attaches_and_ctrl_bracket_detaches() {
        let (mut ws, _, _) = workspace_with(&["a"]);
        assert_eq!(ws.focus, Focus::List);
        ws.handle_key(press(KeyCode::Enter));
        assert_eq!(ws.focus, Focus::Terminal);
        ws.handle_key(ctrl(']'));
        assert_eq!(ws.focus, Focus::List);
    }

    #[test]
    fn unix_spelling_of_the_detach_chord_works() {
        let (mut ws, _, _) = workspace_with(&["a"]);
        ws.handle_key(press(KeyCode::Enter));
        ws.handle_key(ctrl('5'));
        assert_eq!(ws.focus, Focus::List);
    }

    #[test]
    fn attach_with_no_sessions_stays_on_the_list() {
        let (mut ws, _, _) = workspace_with(&[]);
        ws.handle_key(press(KeyCode::Enter));
        assert_eq!(ws.focus, Focus::List);
    }

    #[test]
    fn keys_reach_the_child_only_when_attached() {
        let (mut ws, states, _) = workspace_with(&["a", "b"]);
        // On the list, 'j' navigates and nothing is written.
        ws.handle_key(press(KeyCode::Char('j')));
        assert!(states.iter().all(|s| s.lock().unwrap().written.is_empty()));
        // Attached to "b", keys go to b only.
        ws.handle_key(press(KeyCode::Enter));
        ws.handle_key(press(KeyCode::Char('x')));
        ws.handle_key(press(KeyCode::Char('j')));
        assert!(states[0].lock().unwrap().written.is_empty());
        assert_eq!(states[1].lock().unwrap().written, b"xj");
    }

    #[test]
    fn the_detach_chord_is_not_forwarded() {
        let (mut ws, states, _) = workspace_with(&["a"]);
        ws.handle_key(press(KeyCode::Enter));
        ws.handle_key(ctrl(']'));
        assert!(states[0].lock().unwrap().written.is_empty());
    }

    #[test]
    fn a_key_after_exit_returns_to_the_list_without_writing() {
        let (mut ws, states, _) = workspace_with(&["a"]);
        ws.handle_key(press(KeyCode::Enter));
        states[0].lock().unwrap().exit = exited(true);
        ws.poll_exits();
        ws.handle_key(press(KeyCode::Char('z')));
        assert_eq!(ws.focus, Focus::List);
        assert!(states[0].lock().unwrap().written.is_empty());
    }

    #[test]
    fn n_starts_a_shell_and_agent_keys_start_agents() {
        let (mut ws, _, _) = workspace_with(&["a"]);
        assert_eq!(
            ws.handle_key(press(KeyCode::Char('n'))),
            Outcome::Spawn(NewSession::Shell)
        );
        assert_eq!(
            ws.handle_key(press(KeyCode::Char('c'))),
            Outcome::Spawn(NewSession::Agent("claude".into()))
        );
        assert_eq!(ws.handle_key(press(KeyCode::Char('z'))), Outcome::Continue);
    }

    #[test]
    fn agent_key_does_nothing_when_no_agent_is_available() {
        let (mut ws, _, _) = workspace_with(&["a"]);
        ws.agents.clear();
        assert_eq!(ws.handle_key(press(KeyCode::Char('c'))), Outcome::Continue);
    }

    #[test]
    fn closing_a_running_session_needs_confirmation() {
        let (mut ws, states, _) = workspace_with(&["a", "b"]);
        ws.handle_key(press(KeyCode::Char('x')));
        assert_eq!(ws.panes.len(), 2);
        assert_eq!(ws.confirm, Some(Confirm::Close(SessionId(1))));
        ws.handle_key(press(KeyCode::Char('x')));
        assert_eq!(ws.panes.len(), 1);
        assert!(states[0].lock().unwrap().killed);
        assert_eq!(ws.panes[0].session.name, "b");
        assert_eq!(ws.confirm, None);
    }

    #[test]
    fn any_other_key_cancels_a_pending_close() {
        let (mut ws, states, _) = workspace_with(&["a"]);
        ws.handle_key(press(KeyCode::Char('x')));
        ws.handle_key(press(KeyCode::Char('j')));
        ws.handle_key(press(KeyCode::Char('x')));
        // The second x starts a fresh confirmation instead of closing.
        assert_eq!(ws.panes.len(), 1);
        assert!(!states[0].lock().unwrap().killed);
    }

    #[test]
    fn an_ended_session_closes_without_confirmation() {
        let (mut ws, states, _) = workspace_with(&["a"]);
        states[0].lock().unwrap().exit = exited(false);
        ws.poll_exits();
        ws.handle_key(press(KeyCode::Char('x')));
        assert!(ws.panes.is_empty());
        assert_eq!(ws.selected, 0);
    }

    #[test]
    fn quitting_with_running_sessions_needs_confirmation() {
        let (mut ws, _, _) = workspace_with(&["a"]);
        assert_eq!(ws.handle_key(press(KeyCode::Char('q'))), Outcome::Continue);
        assert_eq!(ws.confirm, Some(Confirm::Quit));
        assert_eq!(ws.handle_key(press(KeyCode::Char('q'))), Outcome::Quit);
    }

    #[test]
    fn quitting_with_nothing_running_is_immediate() {
        let (mut ws, _, _) = workspace_with(&[]);
        assert_eq!(ws.handle_key(press(KeyCode::Char('q'))), Outcome::Quit);
    }

    #[test]
    fn unique_names_get_numeric_suffixes() {
        let (ws, _, _) = workspace_with(&["shell", "shell-2"]);
        assert_eq!(ws.unique_name("claude"), "claude");
        assert_eq!(ws.unique_name("shell"), "shell-3");
    }

    #[test]
    fn events_update_only_the_matching_session() {
        let (mut ws, _, _) = workspace_with(&["a", "b"]);
        ws.handle_event(&state_event(2, SessionState::Working));
        assert_eq!(ws.panes[0].session.state, SessionState::Idle);
        assert_eq!(ws.panes[1].session.state, SessionState::Working);
        ws.handle_event(&state_event(99, SessionState::Working)); // unknown: ignored
    }

    #[test]
    fn output_is_routed_by_session_id() {
        let (mut ws, _, _) = workspace_with(&["a", "b"]);
        ws.handle_output(SessionId(2), b"hi");
        let cell = |i: usize| ws.panes[i].emulator().cell(0, 0).unwrap().symbol;
        assert_eq!(cell(0), " ");
        assert_eq!(cell(1), "h");
    }

    #[test]
    fn a_background_session_that_needs_you_notifies() {
        let (mut ws, _, notes) = workspace_with(&["a", "b"]);
        ws.handle_event(&Event::now(
            SessionId(2),
            EventKind::Message {
                text: "Permission needed: Bash rm -rf x".into(),
            },
        ));
        ws.handle_event(&state_event(2, SessionState::Waiting));
        let sent = notes.0.lock().unwrap().clone();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, "b needs you");
        assert_eq!(sent[0].1, "Permission needed: Bash rm -rf x");
    }

    #[test]
    fn the_attached_session_does_not_notify() {
        let (mut ws, _, notes) = workspace_with(&["a"]);
        ws.handle_key(press(KeyCode::Enter));
        ws.handle_event(&state_event(1, SessionState::Waiting));
        assert!(notes.0.lock().unwrap().is_empty());
        // Once detached, the same transition does notify.
        ws.handle_key(ctrl(']'));
        ws.handle_event(&state_event(1, SessionState::Working));
        ws.handle_event(&state_event(1, SessionState::Waiting));
        assert_eq!(notes.0.lock().unwrap().len(), 1);
    }

    #[test]
    fn repeated_state_does_not_renotify_and_working_is_quiet() {
        let (mut ws, _, notes) = workspace_with(&["a"]);
        ws.handle_event(&state_event(1, SessionState::Working));
        assert!(notes.0.lock().unwrap().is_empty());
        ws.handle_event(&state_event(1, SessionState::Waiting));
        ws.handle_event(&state_event(1, SessionState::Waiting));
        assert_eq!(notes.0.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_background_exit_notifies_with_the_right_headline() {
        let (mut ws, states, notes) = workspace_with(&["a", "b"]);
        states[1].lock().unwrap().exit = exited(false);
        ws.poll_exits();
        ws.poll_exits(); // a second poll must not notify again
        let sent = notes.0.lock().unwrap().clone();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, "b failed");
        assert_eq!(ws.panes[1].session.state, SessionState::Error);
    }

    #[test]
    fn resize_reaches_every_session() {
        let (mut ws, states, _) = workspace_with(&["a", "b"]);
        let screen = Rect::new(0, 0, 120, 40);
        ws.resize(screen);
        let expected = layout::pty_size(screen, false);
        for s in &states {
            assert_eq!(s.lock().unwrap().size, Some(expected));
        }
    }

    #[test]
    fn kill_all_kills_everything() {
        let (mut ws, states, _) = workspace_with(&["a", "b"]);
        ws.kill_all();
        assert!(states.iter().all(|s| s.lock().unwrap().killed));
    }

    #[test]
    fn notices_clear_on_the_next_key() {
        let (mut ws, _, _) = workspace_with(&["a"]);
        ws.notice = Some("oops".into());
        ws.handle_key(press(KeyCode::Char('j')));
        assert_eq!(ws.notice, None);
    }

    // ---- while you were away

    fn event_at(id: u64, secs_ago: i64, kind: EventKind) -> Event {
        Event {
            session_id: SessionId(id),
            ts: OffsetDateTime::now_utc() - time::Duration::seconds(secs_ago),
            kind,
        }
    }

    fn edit_at(id: u64, secs_ago: i64, path: &str) -> Event {
        event_at(id, secs_ago, EventKind::FileEdit { path: path.into() })
    }

    /// A workspace whose only session was last looked at a minute ago.
    fn away_fixture() -> Fixture {
        let mut f = fixture(&["a"]);
        f.ws.panes[0].last_seen = OffsetDateTime::now_utc() - time::Duration::seconds(60);
        f
    }

    #[test]
    fn reattaching_after_activity_shows_what_happened() {
        let mut f = away_fixture();
        f.ws.handle_event(&edit_at(1, 30, "src/a.rs"));
        f.ws.handle_key(press(KeyCode::Enter));
        let overlay = f.ws.overlay.clone().unwrap();
        assert!(overlay.title.contains("while you were away"), "{overlay:?}");
        assert!(
            overlay
                .lines
                .iter()
                .any(|l| l.contains("Edited 1 file(s): src/a.rs")),
            "{overlay:?}"
        );
        assert_eq!(f.ws.focus, Focus::Terminal);
    }

    #[test]
    fn a_quiet_absence_shows_nothing() {
        let mut f = away_fixture();
        f.ws.handle_key(press(KeyCode::Enter));
        assert_eq!(f.ws.overlay, None);
    }

    #[test]
    fn a_very_short_absence_shows_nothing() {
        let mut f = fixture(&["a"]); // last seen just now
        f.ws.handle_event(&edit_at(1, 0, "a.rs"));
        f.ws.handle_key(press(KeyCode::Enter));
        assert_eq!(f.ws.overlay, None);
    }

    #[test]
    fn activity_before_the_last_look_is_not_repeated() {
        let mut f = away_fixture();
        f.ws.handle_event(&edit_at(1, 120, "old.rs")); // before last_seen
        f.ws.handle_key(press(KeyCode::Enter));
        assert_eq!(f.ws.overlay, None);
    }

    #[test]
    fn any_key_dismisses_the_summary_without_reaching_the_session() {
        let mut f = away_fixture();
        f.ws.handle_event(&edit_at(1, 30, "a.rs"));
        f.ws.handle_key(press(KeyCode::Enter));
        assert!(f.ws.overlay.is_some());
        f.ws.handle_key(press(KeyCode::Char('z')));
        assert_eq!(f.ws.overlay, None);
        assert!(f.states[0].lock().unwrap().written.is_empty());
        // The next key is typed into the session as usual.
        f.ws.handle_key(press(KeyCode::Char('z')));
        assert_eq!(f.states[0].lock().unwrap().written, b"z");
    }

    #[test]
    fn detaching_marks_everything_as_seen() {
        let mut f = away_fixture();
        f.ws.handle_key(press(KeyCode::Enter));
        f.ws.handle_event(&edit_at(1, 0, "a.rs")); // happens while attached
        f.ws.handle_key(ctrl(']'));
        assert!(f.ws.panes[0].away_summary().is_quiet());
        f.ws.handle_key(press(KeyCode::Enter));
        assert_eq!(f.ws.overlay, None);
    }

    #[test]
    fn the_summary_mentions_a_trust_problem() {
        let mut f = away_fixture();
        let command = EventKind::Command {
            command: "git reset --hard".into(),
        };
        f.ws.handle_event(&event_at(1, 30, command));
        f.ws.tick();
        f.ws.handle_key(press(KeyCode::Enter));
        let overlay = f.ws.overlay.clone().unwrap();
        assert!(
            overlay.lines.iter().any(|l| l.starts_with("Trust: RED")),
            "{overlay:?}"
        );
    }

    // ---- panel

    #[test]
    fn toggling_the_panel_resizes_every_terminal() {
        let (mut ws, states, _) = workspace_with(&["a", "b"]);
        assert!(!ws.show_panel);
        ws.handle_key(press(KeyCode::Char('t')));
        assert!(ws.show_panel);
        let with_panel = layout::pty_size(SCREEN, true);
        assert!(with_panel.cols < layout::pty_size(SCREEN, false).cols);
        for s in &states {
            assert_eq!(s.lock().unwrap().size, Some(with_panel));
        }
        ws.handle_key(press(KeyCode::Tab));
        assert!(!ws.show_panel);
        assert_eq!(
            states[0].lock().unwrap().size,
            Some(layout::pty_size(SCREEN, false))
        );
    }

    // ---- bus

    #[test]
    fn adding_a_session_publishes_its_description() {
        let mut f = fixture(&["alpha"]);
        let published = f.published();
        assert_eq!(published.len(), 1);
        assert!(matches!(
            &published[0].kind,
            EventKind::Session { name, agent, .. } if name == "alpha" && agent == "generic"
        ));
    }

    #[test]
    fn agent_events_and_exits_are_published_but_stray_events_are_not() {
        let mut f = fixture(&["a"]);
        f.published();
        f.ws.handle_event(&state_event(1, SessionState::Working));
        f.ws.handle_event(&state_event(99, SessionState::Working));
        f.states[0].lock().unwrap().exit = exited(true);
        f.ws.poll_exits();
        let kinds: Vec<_> = f.published().into_iter().map(|e| e.kind).collect();
        assert_eq!(
            kinds,
            [
                EventKind::State(SessionState::Working),
                EventKind::State(SessionState::Done),
            ]
        );
    }

    // ---- trust

    fn command_event(id: u64, command: &str) -> Event {
        Event::now(
            SessionId(id),
            EventKind::Command {
                command: command.into(),
            },
        )
    }

    #[test]
    fn a_dangerous_command_turns_the_badge_red_and_is_published_once() {
        let mut f = fixture(&["a"]);
        f.published();
        f.ws.handle_event(&command_event(1, "git push --force"));
        f.ws.tick();
        f.ws.tick(); // nothing changed, so nothing more is published
        assert_eq!(f.ws.panes[0].trust.badge, Badge::Red);
        let trust: Vec<_> = f
            .published()
            .into_iter()
            .filter(|e| matches!(e.kind, EventKind::Trust { .. }))
            .collect();
        assert_eq!(trust.len(), 1);
        match &trust[0].kind {
            EventKind::Trust { badge, findings } => {
                assert_eq!(*badge, Badge::Red);
                assert_eq!(findings[0].severity, Severity::Alert);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn a_background_session_turning_red_notifies_once() {
        let mut f = fixture(&["a"]);
        f.ws.handle_event(&command_event(1, "rm -rf /"));
        f.ws.tick();
        f.ws.handle_event(&command_event(1, "git reset --hard"));
        f.ws.tick();
        let sent = f.notes.0.lock().unwrap().clone();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, "a: trust alert");
        assert!(sent[0].1.contains("rm -rf /"));
    }

    #[test]
    fn the_attached_session_turning_red_does_not_notify() {
        let mut f = fixture(&["a"]);
        f.ws.handle_key(press(KeyCode::Enter));
        f.ws.handle_event(&command_event(1, "rm -rf /"));
        f.ws.tick();
        assert_eq!(f.ws.panes[0].trust.badge, Badge::Red);
        assert!(f.notes.0.lock().unwrap().is_empty());
    }

    #[test]
    fn a_diff_feeds_the_trust_verdict() {
        let mut f = fixture(&["a"]);
        let diff = Diff {
            files: vec![FileChange::new("tests/a_test.py", ChangeKind::Deleted)],
        };
        f.ws.panes[0].git = GitState::Ready(dummy_baseline());
        f.ws.finish_diff(SessionId(1), Some(diff));
        f.ws.tick();
        assert_eq!(f.ws.panes[0].trust.badge, Badge::Red);
    }

    // ---- diff scheduling

    struct FakeSource;

    impl DiffSource for FakeSource {
        fn diff(&self) -> std::io::Result<Diff> {
            Ok(Diff::default())
        }
    }

    fn dummy_baseline() -> Arc<dyn DiffSource> {
        Arc::new(FakeSource)
    }

    #[test]
    fn state_changes_that_need_attention_request_a_diff() {
        let mut f = fixture(&["a"]);
        f.ws.panes[0].git = GitState::Ready(dummy_baseline());
        f.ws.handle_event(&state_event(1, SessionState::Working));
        assert!(f.ws.take_diff_requests().is_empty());
        f.ws.handle_event(&state_event(1, SessionState::Waiting));
        let jobs = f.ws.take_diff_requests();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].0, SessionId(1));
    }

    #[test]
    fn requests_without_a_ready_baseline_are_dropped() {
        let mut f = fixture(&["a", "b"]);
        f.ws.panes[1].git = GitState::NotARepo;
        f.ws.handle_event(&state_event(1, SessionState::Waiting));
        f.ws.handle_event(&state_event(2, SessionState::Waiting));
        assert!(f.ws.take_diff_requests().is_empty());
    }

    #[test]
    fn one_diff_runs_at_a_time_and_a_pending_request_is_retried() {
        let mut f = fixture(&["a"]);
        f.ws.panes[0].git = GitState::Ready(dummy_baseline());
        f.ws.handle_key(press(KeyCode::Char('r')));
        f.ws.handle_key(press(KeyCode::Char('r'))); // merged with the first
        assert_eq!(f.ws.take_diff_requests().len(), 1);

        f.ws.handle_key(press(KeyCode::Char('r')));
        assert!(f.ws.take_diff_requests().is_empty()); // still running
        f.ws.finish_diff(SessionId(1), Some(Diff::default()));
        assert_eq!(f.ws.take_diff_requests().len(), 1); // retried
        f.ws.finish_diff(SessionId(1), None); // failure clears the flag too
        assert!(!f.ws.panes[0].diff_in_flight);
        assert!(f.ws.take_diff_requests().is_empty());
    }

    #[test]
    fn detaching_requests_a_diff() {
        let mut f = fixture(&["a"]);
        f.ws.panes[0].git = GitState::Ready(dummy_baseline());
        f.ws.handle_key(press(KeyCode::Enter));
        f.ws.handle_key(ctrl(']'));
        assert_eq!(f.ws.take_diff_requests().len(), 1);
    }

    #[test]
    fn working_sessions_are_re_diffed_periodically() {
        let mut f = fixture(&["a", "b"]);
        for pane in &mut f.ws.panes {
            pane.git = GitState::Ready(dummy_baseline());
        }
        f.ws.handle_event(&state_event(1, SessionState::Working));
        for _ in 0..DIFF_REFRESH_TICKS {
            f.ws.tick();
        }
        let jobs = f.ws.take_diff_requests();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].0, SessionId(1));
    }

    #[test]
    fn baseline_results_are_recorded() {
        let mut f = fixture(&["a", "b"]);
        f.ws.set_baseline(SessionId(1), None);
        f.ws.set_baseline(SessionId(2), Some(dummy_baseline()));
        assert!(matches!(f.ws.panes[0].git, GitState::NotARepo));
        assert!(matches!(f.ws.panes[1].git, GitState::Ready(_)));
    }

    #[test]
    fn silent_claude_hooks_turn_the_badge_yellow_until_a_report_arrives() {
        let mut f = fixture(&[]);
        let state = Arc::new(Mutex::new(FakeState::default()));
        let mut session = Session::new(
            SessionId(9),
            "claude",
            AgentKind::Claude,
            PathBuf::from("."),
        );
        session.started_at = std::time::SystemTime::now() - std::time::Duration::from_secs(120);
        let size = layout::pty_size(SCREEN, false);
        f.ws.add_pane(Pane::new(session, Box::new(FakePty(state)), size), false);

        f.ws.handle_output(SessionId(9), &[b'x'; 5000]);
        f.ws.tick();
        assert_eq!(f.ws.panes[0].trust.badge, Badge::Yellow);
        assert_eq!(f.ws.panes[0].trust.findings[0].rule, "hooks-silent");

        f.ws.handle_event(&state_event(9, SessionState::Working));
        f.ws.tick();
        assert_eq!(f.ws.panes[0].trust.badge, Badge::Green);
    }
}
