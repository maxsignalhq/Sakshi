//! The TUI application: owns the terminal, the event loop and process launching.

use std::io;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use ratatui::crossterm::event::{self, Event as TermEvent, KeyEventKind};
use ratatui::layout::Rect;
use ratatui::DefaultTerminal;
use sakshi_adapters::LaunchContext;
use sakshi_core::{AgentKind, Event, Session, SessionId};
use sakshi_ipc::{Endpoint, IpcServer, RuntimeDir, ENDPOINT_ENV, SESSION_ENV};
use sakshi_pty::{OutputReceiver, PtyError, PtySession, SpawnConfig, TermSize};
use tokio::sync::mpsc;

use crate::notify::{DesktopNotifier, NoopNotifier, Notifier};
use crate::pane::Pane;
use crate::workspace::{AgentInfo, NewSession, Outcome, Workspace};
use crate::{layout, ui};

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
}

/// Everything the event loop reacts to, funnelled into one channel.
enum AppEvent {
    Input(TermEvent),
    Output(SessionId, Vec<u8>),
    Agent(Event),
    AgentsProbed(Vec<AgentInfo>),
    Tick,
}

/// Run the TUI until the user quits. Restores the terminal on exit.
pub async fn run(config: RunConfig) -> Result<(), TuiError> {
    // Set up IPC before touching the terminal so failures print normally.
    let runtime = RuntimeDir::create()?;
    let endpoint = runtime.endpoint();
    let server = IpcServer::start(&endpoint)?;
    let mut terminal = ratatui::try_init()?;
    let result = run_loop(&mut terminal, config, runtime, endpoint, server).await;
    ratatui::restore();
    result
}

/// Starts sessions: knows the IPC endpoint and where to send their output.
struct Launcher {
    runtime: RuntimeDir,
    endpoint: Endpoint,
    events: mpsc::UnboundedSender<AppEvent>,
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
        let env = vec![
            (SESSION_ENV.to_string(), id.to_string()),
            (ENDPOINT_ENV.to_string(), self.endpoint.to_string()),
        ];
        let Some(adapter) = sakshi_adapters::for_program(program) else {
            return (AgentKind::Generic, user_args, env);
        };
        let prefix = std::env::current_exe()
            .and_then(|hook_exe| {
                adapter.launch_args(&LaunchContext {
                    hook_exe: &hook_exe,
                    runtime_dir: self.runtime.path(),
                })
            })
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
        let session = Session::new(id, name, kind, cwd);
        Ok(Pane::new(session, Box::new(pty), size))
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
}

fn pty_size_of(terminal: &DefaultTerminal) -> io::Result<TermSize> {
    let screen = terminal.size()?;
    Ok(layout::pty_size(Rect::new(
        0,
        0,
        screen.width,
        screen.height,
    )))
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

    let notifier: Box<dyn Notifier> = if config.notifications {
        Box::new(DesktopNotifier)
    } else {
        Box::new(NoopNotifier)
    };
    let size = pty_size_of(terminal)?;
    let mut ws = Workspace::new(size, Vec::new(), notifier);

    let name = ws.unique_name(&config.name);
    let first = launcher.spawn(name, config.program, config.args, config.cwd.clone(), size)?;
    ws.add_pane(first, true);

    loop {
        terminal.draw(|frame| ui::draw(frame, &ws))?;
        let Some(first) = rx.recv().await else { break };
        let mut quit = handle(&mut ws, &launcher, &config.shell, first);
        // Coalesce whatever else is already queued into one redraw.
        while !quit {
            match rx.try_recv() {
                Ok(next) => quit = handle(&mut ws, &launcher, &config.shell, next),
                Err(_) => break,
            }
        }
        if quit {
            break;
        }
    }

    ws.kill_all();
    Ok(())
}

/// Installed agents the user can start from the list.
fn probe_agents() -> Vec<AgentInfo> {
    sakshi_adapters::available()
        .iter()
        .filter_map(|adapter| {
            let program = adapter.default_program();
            Some(AgentInfo {
                name: adapter.display_name().to_string(),
                program: program.to_string(),
                key: program.chars().next()?.to_ascii_lowercase(),
            })
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
            ws.resize(layout::pty_size(Rect::new(0, 0, width, height)));
        }
        AppEvent::Input(_) => {}
        AppEvent::Output(id, bytes) => ws.handle_output(id, &bytes),
        AppEvent::Agent(event) => ws.handle_event(&event),
        AppEvent::AgentsProbed(agents) => ws.agents = agents,
        AppEvent::Tick => ws.poll_exits(),
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
    match launcher.spawn(name, program.clone(), Vec::new(), cwd, ws.size) {
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

    #[test]
    fn probing_yields_keys_from_program_names() {
        // Whatever is installed, every offered agent has a lowercase key.
        for agent in probe_agents() {
            assert_eq!(
                agent.key,
                agent.program.chars().next().unwrap().to_ascii_lowercase()
            );
        }
    }
}
