//! The set of sessions and what the user is doing with them.
//!
//! Pure state: no I/O happens here. Key presses come in, and the result says
//! what the caller must do (quit, or start a new session).

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use sakshi_core::{Event, SessionId, SessionState};
use sakshi_pty::TermSize;

use crate::notify::Notifier;
use crate::pane::Pane;

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
    pub size: TermSize,
    pub agents: Vec<AgentInfo>,
    notifier: Box<dyn Notifier>,
}

/// Whether `key` is the detach chord, Ctrl+].
///
/// Unix terminals deliver 0x1D, which crossterm reports as Ctrl+5.
fn is_detach_chord(key: KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char(']') | KeyCode::Char('5'))
}

impl Workspace {
    pub fn new(size: TermSize, agents: Vec<AgentInfo>, notifier: Box<dyn Notifier>) -> Self {
        Self {
            panes: Vec::new(),
            selected: 0,
            focus: Focus::List,
            confirm: None,
            notice: None,
            size,
            agents,
            notifier,
        }
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
        if !self.panes.is_empty() {
            self.focus = Focus::Terminal;
        }
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

    /// Route child output to its pane.
    pub fn handle_output(&mut self, id: SessionId, bytes: &[u8]) {
        if let Some(pane) = self.panes.iter_mut().find(|p| p.session.id == id) {
            pane.feed(bytes);
        }
    }

    /// Apply an agent event to its session and notify if the session now needs
    /// the user. Events for unknown sessions are ignored.
    pub fn handle_event(&mut self, event: &Event) {
        let Some(index) = self
            .panes
            .iter()
            .position(|p| p.session.id == event.session_id)
        else {
            return;
        };
        if let Some(state) = self.panes[index].apply(event) {
            self.notify_if_needed(index, state);
        }
    }

    /// Detect processes that have exited since the last call.
    pub fn poll_exits(&mut self) {
        for index in 0..self.panes.len() {
            match self.panes[index].poll_exit() {
                Ok(Some(state)) => self.notify_if_needed(index, state),
                Ok(None) => {}
                Err(e) => tracing::warn!("checking session exit failed: {e}"),
            }
        }
    }

    pub fn resize(&mut self, size: TermSize) {
        self.size = size;
        for pane in &mut self.panes {
            pane.resize(size);
        }
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
    use sakshi_core::{AgentKind, EventKind, Session};
    use sakshi_pty::{ExitInfo, PtyError};

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

    const SIZE: TermSize = TermSize { rows: 10, cols: 40 };

    pub fn fake_pane(id: u64, name: &str) -> (Pane, Arc<Mutex<FakeState>>) {
        let state = Arc::new(Mutex::new(FakeState::default()));
        let session = Session::new(SessionId(id), name, AgentKind::Generic, PathBuf::from("."));
        (
            Pane::new(session, Box::new(FakePty(state.clone())), SIZE),
            state,
        )
    }

    pub fn workspace_with(names: &[&str]) -> (Workspace, Vec<Arc<Mutex<FakeState>>>, Notes) {
        let notes = Notes::default();
        let agents = vec![AgentInfo {
            name: "Claude Code".into(),
            program: "claude".into(),
            key: 'c',
        }];
        let mut ws = Workspace::new(SIZE, agents, Box::new(notes.clone()));
        let mut states = Vec::new();
        for (i, name) in names.iter().enumerate() {
            let (pane, state) = fake_pane(i as u64 + 1, name);
            ws.add_pane(pane, false);
            states.push(state);
        }
        (ws, states, notes)
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
        states[0].lock().unwrap().exit = Some(ExitInfo {
            success: true,
            code: 0,
        });
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
        let mut ws = Workspace::new(SIZE, Vec::new(), Box::new(Notes::default()));
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
        states[0].lock().unwrap().exit = Some(ExitInfo {
            success: false,
            code: 1,
        });
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
        states[1].lock().unwrap().exit = Some(ExitInfo {
            success: false,
            code: 3,
        });
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
        let size = TermSize { rows: 30, cols: 99 };
        ws.resize(size);
        assert_eq!(ws.size, size);
        for s in &states {
            assert_eq!(s.lock().unwrap().size, Some(size));
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
}
