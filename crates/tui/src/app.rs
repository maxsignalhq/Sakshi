//! The TUI application: owns the terminal, the event loop and process launching.

use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use ratatui::crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event as TermEvent, KeyCode, KeyEvent,
    KeyEventKind, KeyModifiers,
};
use ratatui::crossterm::execute;
use ratatui::layout::Rect;
use ratatui::DefaultTerminal;
use sakshi_adapters::LaunchContext;
use sakshi_core::{AgentKind, Diff, Event, EventBus, Session, SessionId, TrustConfig};
use sakshi_git::Baseline;
use sakshi_ipc::{Endpoint, IpcServer, RuntimeDir, ENDPOINT_ENV, SESSION_ENV};
use sakshi_pty::{OutputReceiver, PtyError, PtySession, SpawnConfig, TermSize};
use sakshi_store::{sessions_dir, LogSet};
use tokio::sync::mpsc;

use crate::notify::{DesktopNotifier, NoopNotifier, Notifier};
use crate::pane::{DiffSource, Pane};
use crate::ui;
use crate::workspace::{AgentInfo, NewSession, Outcome, Workspace};

/// Errors that end the TUI.
#[derive(Debug, thiserror::Error)]
pub enum TuiError {
    /// Terminal setup or drawing failed.
    #[error("terminal error: {0}")]
    Io(#[from] io::Error),
    /// The first session's PTY failed.
    #[error(transparent)]
    Pty(#[from] PtyError),
}

/// What to run, and how.
#[derive(Debug, Clone)]
pub struct RunConfig {
    /// Name shown in the session list for the first session.
    pub name: String,
    /// Program of the first session.
    pub program: String,
    /// Arguments of the first session.
    pub args: Vec<String>,
    /// Working directory of the first session.
    pub cwd: PathBuf,
    /// Program for sessions started with `n`.
    pub shell: String,
    /// Whether to show desktop notifications.
    pub notifications: bool,
    /// Where to keep activity logs; `None` turns logging off.
    pub data_dir: Option<PathBuf>,
    /// The user's adjustments to the trust rules.
    pub trust: TrustConfig,
}

/// Everything the event loop reacts to, funnelled into one channel.
enum AppEvent {
    Input(TermEvent),
    Output(SessionId, Vec<u8>),
    Agent(Event),
    AgentsProbed(Vec<AgentInfo>),
    /// A session's repository snapshot finished (`None`: not a repository).
    Baseline(SessionId, Option<Arc<dyn DiffSource>>),
    /// A requested diff finished (`None`: it failed).
    Diff(SessionId, Option<Diff>),
    Tick,
}

/// Run the TUI until the user quits. Restores the terminal on exit.
pub async fn run(config: RunConfig) -> Result<(), TuiError> {
    // Set up IPC before touching the terminal so failures print normally.
    let runtime = RuntimeDir::create()?;
    let endpoint = runtime.endpoint();
    let server = IpcServer::start(&endpoint)?;
    let mut terminal = ratatui::try_init()?;
    // Ask the terminal to mark pastes, so a multi-line paste is text and not a
    // series of Enter presses. Best effort: not every terminal supports it.
    let _ = execute!(io::stdout(), EnableBracketedPaste);
    let result = run_loop(&mut terminal, config, runtime, endpoint, server).await;
    let _ = execute!(io::stdout(), DisableBracketedPaste);
    ratatui::restore();
    result
}

/// Starts sessions: knows the IPC endpoint and where to send their output.
struct Launcher {
    runtime: RuntimeDir,
    endpoint: Endpoint,
    events: mpsc::UnboundedSender<AppEvent>,
    trust: Arc<TrustConfig>,
}

impl Launcher {
    /// Arguments and environment that make `program` report to Sakshi.
    ///
    /// Fails open: if the adapter cannot prepare its hooks, the agent still
    /// runs, just without live state.
    fn plan(
        &self,
        program: &str,
        user_args: Vec<String>,
        id: SessionId,
    ) -> (AgentKind, Vec<String>, Vec<(String, String)>) {
        let mut env = vec![
            (SESSION_ENV.to_string(), id.to_string()),
            (ENDPOINT_ENV.to_string(), self.endpoint.to_string()),
        ];
        let Some(adapter) = sakshi_adapters::for_program(program) else {
            return (AgentKind::Generic, user_args, env);
        };
        let prefix = match std::env::current_exe() {
            Ok(hook_exe) => {
                let ctx = LaunchContext {
                    hook_exe: &hook_exe,
                    runtime_dir: self.runtime.path(),
                    program,
                    user_args: &user_args,
                };
                env.extend(adapter.launch_env(&ctx));
                adapter.launch_args(&ctx)
            }
            Err(e) => Err(e),
        }
        .unwrap_or_else(|e| {
            tracing::warn!("could not set up {} hooks: {e}", adapter.display_name());
            Vec::new()
        });
        let args = prefix.into_iter().chain(user_args).collect();
        (adapter.kind(), args, env)
    }

    /// Start a session and begin forwarding its output.
    fn spawn(
        &self,
        name: String,
        program: String,
        args: Vec<String>,
        cwd: PathBuf,
        size: TermSize,
    ) -> Result<Pane, PtyError> {
        let id = SessionId::generate();
        let (kind, args, env) = self.plan(&program, args, id);
        let (pty, output) = PtySession::spawn(SpawnConfig {
            program,
            args,
            cwd: Some(cwd.clone()),
            env,
            size,
        })?;
        self.forward_output(id, output);
        self.capture_baseline(id, cwd.clone());
        let session = Session::new(id, name, kind, cwd);
        Ok(Pane::new(session, Box::new(pty), size).with_trust_config(self.trust.clone()))
    }

    fn forward_output(&self, id: SessionId, mut output: OutputReceiver) {
        let events = self.events.clone();
        tokio::spawn(async move {
            while let Some(bytes) = output.recv().await {
                if events.send(AppEvent::Output(id, bytes)).is_err() {
                    return;
                }
            }
        });
    }

    /// Snapshot the session's repository off the UI thread: git can be slow.
    fn capture_baseline(&self, id: SessionId, cwd: PathBuf) {
        let events = self.events.clone();
        tokio::task::spawn_blocking(move || {
            let baseline = Baseline::capture(&cwd).map(|b| Arc::new(b) as Arc<dyn DiffSource>);
            let _ = events.send(AppEvent::Baseline(id, baseline));
        });
    }
}

/// Write every event published on `bus` to the activity log, until the bus is
/// dropped. Runs on its own thread because it does blocking file I/O.
fn spawn_log_writer(bus: &EventBus, data_dir: PathBuf) -> io::Result<thread::JoinHandle<()>> {
    let mut events = bus.subscribe();
    thread::Builder::new()
        .name("sakshi-log".into())
        .spawn(move || {
            let mut logs = LogSet::new(sessions_dir(&data_dir));
            while let Some(event) = events.blocking_recv() {
                logs.record(&event);
            }
        })
}

fn screen_rect(terminal: &DefaultTerminal) -> io::Result<Rect> {
    let size = terminal.size()?;
    Ok(Rect::new(0, 0, size.width, size.height))
}

async fn run_loop(
    terminal: &mut DefaultTerminal,
    config: RunConfig,
    runtime: RuntimeDir,
    endpoint: Endpoint,
    mut server: IpcServer,
) -> Result<(), TuiError> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let launcher = Launcher {
        runtime,
        endpoint,
        events: tx.clone(),
        trust: Arc::new(config.trust.clone()),
    };

    // crossterm's reader blocks, so it gets its own thread. The thread ends
    // when the process does; it cannot be interrupted while waiting for input.
    let input_tx = tx.clone();
    thread::Builder::new()
        .name("sakshi-input".into())
        .spawn(move || {
            while let Ok(ev) = event::read() {
                if input_tx.send(AppEvent::Input(ev)).is_err() {
                    break;
                }
            }
        })?;

    let ipc_tx = tx.clone();
    tokio::spawn(async move {
        while let Some(ev) = server.recv().await {
            if ipc_tx.send(AppEvent::Agent(ev)).is_err() {
                return;
            }
        }
    });

    let tick_tx = tx.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        loop {
            tick.tick().await;
            if tick_tx.send(AppEvent::Tick).is_err() {
                return;
            }
        }
    });

    // Probing runs the agents' `--version`, so it must not block the UI.
    let probe_tx = tx.clone();
    tokio::task::spawn_blocking(move || {
        let _ = probe_tx.send(AppEvent::AgentsProbed(probe_agents()));
    });

    let bus = EventBus::new();
    let log_thread = match config.data_dir.clone() {
        Some(dir) => spawn_log_writer(&bus, dir).ok(),
        None => None,
    };

    let notifier: Box<dyn Notifier> = if config.notifications {
        Box::new(DesktopNotifier)
    } else {
        Box::new(NoopNotifier)
    };
    let mut ws = Workspace::new(screen_rect(terminal)?, Vec::new(), notifier, bus);

    let name = ws.unique_name(&config.name);
    let first = launcher.spawn(
        name,
        config.program,
        config.args,
        config.cwd.clone(),
        ws.pty_size(),
    )?;
    ws.add_pane(first, true);

    loop {
        terminal.draw(|frame| ui::draw(frame, &ws))?;
        let Some(first) = rx.recv().await else { break };
        // Coalesce whatever else is already queued into one redraw.
        let mut batch = vec![first];
        while let Ok(next) = rx.try_recv() {
            batch.push(next);
        }
        if cfg!(windows) {
            // The Windows console never reports pastes, only key presses.
            batch = coalesce_pastes(batch);
        }
        let mut quit = false;
        for event in batch {
            if handle(&mut ws, &launcher, &config.shell, event) {
                quit = true;
                break;
            }
        }
        if quit {
            break;
        }
        start_diffs(&mut ws, &tx);
    }

    ws.kill_all();
    // Dropping the workspace drops the last bus handle, which lets the log
    // writer drain its queue and stop; wait for it so the tail is on disk.
    drop(ws);
    if let Some(thread) = log_thread {
        let _ = thread.join();
    }
    Ok(())
}

/// The fewest key presses in one burst that count as a paste.
const PASTE_BURST_MIN: usize = 8;

/// The character a plain text key press stands for, if it is one.
fn typed_char(key: &KeyEvent) -> Option<char> {
    if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
        || key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        return None;
    }
    match key.code {
        KeyCode::Char(c) => Some(c),
        KeyCode::Enter => Some('\n'),
        KeyCode::Tab => Some('\t'),
        _ => None,
    }
}

/// Turn a burst of text key presses into one paste.
///
/// Terminals with bracketed paste say when text is pasted. The Windows
/// console does not: a paste arrives as key presses, and a newline in it would
/// submit whatever the program has so far. A person cannot type eight or more
/// characters, including a line break, between two screen updates, so such a
/// burst is a paste. Key releases inside the burst are dropped with it.
fn coalesce_pastes(events: Vec<AppEvent>) -> Vec<AppEvent> {
    fn flush(out: &mut Vec<AppEvent>, text: &mut String, held: &mut Vec<AppEvent>) {
        if text.chars().count() >= PASTE_BURST_MIN && text.contains('\n') {
            out.push(AppEvent::Input(TermEvent::Paste(std::mem::take(text))));
            held.clear();
        } else {
            out.append(held);
        }
        text.clear();
    }

    let mut out = Vec::with_capacity(events.len());
    let mut text = String::new();
    let mut held: Vec<AppEvent> = Vec::new();
    for event in events {
        let typed = match &event {
            AppEvent::Input(TermEvent::Key(key)) => typed_char(key),
            _ => None,
        };
        let is_release = matches!(
            &event,
            AppEvent::Input(TermEvent::Key(key)) if key.kind == KeyEventKind::Release
        );
        match typed {
            Some(c) => {
                text.push(c);
                held.push(event);
            }
            // A release between the presses of a burst belongs to the burst.
            None if is_release && !held.is_empty() => held.push(event),
            None => {
                flush(&mut out, &mut text, &mut held);
                out.push(event);
            }
        }
    }
    flush(&mut out, &mut text, &mut held);
    out
}

/// Run the diffs the workspace asked for, off the UI thread.
fn start_diffs(ws: &mut Workspace, tx: &mpsc::UnboundedSender<AppEvent>) {
    for (id, source) in ws.take_diff_requests() {
        let tx = tx.clone();
        tokio::task::spawn_blocking(move || {
            let diff = match source.diff() {
                Ok(diff) => Some(diff),
                Err(e) => {
                    tracing::warn!("could not diff session {id}: {e}");
                    None
                }
            };
            let _ = tx.send(AppEvent::Diff(id, diff));
        });
    }
}

/// Installed agents the user can start from the list.
fn probe_agents() -> Vec<AgentInfo> {
    sakshi_adapters::available()
        .iter()
        .map(|adapter| AgentInfo {
            name: adapter.display_name().to_string(),
            program: adapter.default_program().to_string(),
            key: adapter.start_key(),
        })
        .collect()
}

/// React to one event. Returns `true` when the app should quit.
fn handle(ws: &mut Workspace, launcher: &Launcher, shell: &str, ev: AppEvent) -> bool {
    match ev {
        AppEvent::Input(TermEvent::Key(key))
            if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
        {
            match ws.handle_key(key) {
                Outcome::Continue => {}
                Outcome::Quit => return true,
                Outcome::Spawn(what) => start_session(ws, launcher, shell, what),
            }
        }
        AppEvent::Input(TermEvent::Resize(width, height)) => {
            ws.resize(Rect::new(0, 0, width, height));
        }
        AppEvent::Input(TermEvent::Paste(text)) => ws.handle_paste(&text),
        AppEvent::Input(_) => {}
        AppEvent::Output(id, bytes) => ws.handle_output(id, &bytes),
        AppEvent::Agent(event) => ws.handle_event(&event),
        AppEvent::AgentsProbed(agents) => ws.agents = agents,
        AppEvent::Baseline(id, baseline) => ws.set_baseline(id, baseline),
        AppEvent::Diff(id, diff) => ws.finish_diff(id, diff),
        AppEvent::Tick => ws.tick(),
    }
    false
}

/// Start a new session from the list. A failure becomes a footer notice, not
/// a crash: the other sessions must keep running.
fn start_session(ws: &mut Workspace, launcher: &Launcher, shell: &str, what: NewSession) {
    let cwd = ws
        .selected_pane()
        .map(|p| p.session.working_dir.clone())
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    let (program, base) = match what {
        NewSession::Shell => (shell.to_string(), program_stem(shell)),
        NewSession::Agent(program) => {
            let base = program_stem(&program);
            (program, base)
        }
    };
    let name = ws.unique_name(&base);
    match launcher.spawn(name, program.clone(), Vec::new(), cwd, ws.pty_size()) {
        Ok(pane) => ws.add_pane(pane, true),
        Err(e) => ws.notice = Some(format!("Could not start {program}: {e}")),
    }
}

fn program_stem(program: &str) -> String {
    std::path::Path::new(program)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| program.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn program_stem_drops_directory_and_extension() {
        assert_eq!(program_stem("/usr/bin/zsh"), "zsh");
        assert_eq!(program_stem("powershell.exe"), "powershell");
        assert_eq!(program_stem("claude"), "claude");
    }

    fn key(code: KeyCode, kind: KeyEventKind) -> AppEvent {
        AppEvent::Input(TermEvent::Key(KeyEvent::new_with_kind(
            code,
            KeyModifiers::NONE,
            kind,
        )))
    }

    /// Press and release of each character, as the Windows console reports it.
    fn typed(text: &str) -> Vec<AppEvent> {
        text.chars()
            .flat_map(|c| {
                let code = if c == '\n' {
                    KeyCode::Enter
                } else {
                    KeyCode::Char(c)
                };
                [
                    key(code, KeyEventKind::Press),
                    key(code, KeyEventKind::Release),
                ]
            })
            .collect()
    }

    fn describe(events: &[AppEvent]) -> Vec<String> {
        events
            .iter()
            .map(|e| match e {
                AppEvent::Input(TermEvent::Paste(t)) => format!("paste:{t}"),
                AppEvent::Input(TermEvent::Key(k)) => format!("{:?}/{:?}", k.code, k.kind),
                _ => "other".to_string(),
            })
            .collect()
    }

    #[test]
    fn a_burst_with_a_line_break_becomes_one_paste() {
        let out = coalesce_pastes(typed("hello\nworld"));
        assert_eq!(describe(&out), ["paste:hello\nworld"]);
    }

    #[test]
    fn typing_never_becomes_a_paste() {
        // Slow typing arrives a character or two at a time.
        let out = coalesce_pastes(typed("ls\n"));
        assert_eq!(out.len(), 6);
        assert!(describe(&out).iter().all(|d| !d.starts_with("paste")));
        // A long burst with no line break is typed text or a held key.
        let out = coalesce_pastes(typed("aaaaaaaaaaaaaaaa"));
        assert_eq!(out.len(), 32);
    }

    #[test]
    fn a_burst_is_cut_at_other_events_and_other_keys() {
        let mut events = typed("short\n");
        events.push(key(KeyCode::Up, KeyEventKind::Press));
        events.extend(typed("a long enough line\nof pasted text"));
        events.push(AppEvent::Tick);
        let out = coalesce_pastes(events);
        let d = describe(&out);
        assert_eq!(d.len(), 12 + 1 + 1 + 1, "{d:?}");
        assert!(d.contains(&"paste:a long enough line\nof pasted text".to_string()));
        assert_eq!(d.last().map(String::as_str), Some("other"));
    }

    #[test]
    fn modified_keys_break_a_burst() {
        let mut events = typed("abcd\nefgh");
        events.insert(
            4,
            AppEvent::Input(TermEvent::Key(KeyEvent::new(
                KeyCode::Char('c'),
                KeyModifiers::CONTROL,
            ))),
        );
        let out = coalesce_pastes(events);
        assert!(describe(&out).iter().all(|d| !d.starts_with("paste")));
    }

    #[test]
    fn every_agent_has_a_unique_lowercase_key_that_is_not_a_reserved_one() {
        // Keys the session list uses itself; an agent must not shadow them.
        const RESERVED: [char; 8] = ['n', 'q', 'x', 't', 'j', 'k', 'l', 'r'];
        // Checked over every adapter, installed or not, so CI sees all of them.
        let keys: Vec<char> = sakshi_adapters::all()
            .iter()
            .map(|a| a.start_key())
            .collect();
        for key in &keys {
            assert!(key.is_ascii_lowercase(), "{key}");
            assert!(!RESERVED.contains(key), "{key} is reserved");
        }
        let mut unique = keys.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), keys.len(), "{keys:?}");
        // Whatever is installed is offered under its adapter's key.
        for agent in probe_agents() {
            assert!(keys.contains(&agent.key));
        }
    }
}
