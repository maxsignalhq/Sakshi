//! The TUI application: one live terminal session with a session list and footer.

use std::io;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph};
use ratatui::{DefaultTerminal, Frame};
use sakshi_adapters::LaunchContext;
use sakshi_core::{AgentKind, Session, SessionId};
use sakshi_ipc::{Endpoint, IpcServer, RuntimeDir, ENDPOINT_ENV, SESSION_ENV};
use sakshi_pty::{ExitInfo, PtyError, PtySession, SpawnConfig};
use tokio::sync::mpsc;

use crate::emulator::{TerminalEmulator, Vt100Emulator};
use crate::{keys, layout};

/// Errors that end the TUI.
#[derive(Debug, thiserror::Error)]
pub enum TuiError {
    /// Terminal setup or drawing failed.
    #[error("terminal error: {0}")]
    Io(#[from] io::Error),
    /// The session's PTY failed.
    #[error(transparent)]
    Pty(#[from] PtyError),
}

/// What to run in the session.
#[derive(Debug, Clone)]
pub struct RunConfig {
    /// Name shown in the session list.
    pub name: String,
    /// Program to execute.
    pub program: String,
    /// Arguments passed to the program.
    pub args: Vec<String>,
    /// Working directory of the session.
    pub cwd: PathBuf,
}

/// Run the TUI until the user quits. Restores the terminal on exit.
pub async fn run(config: RunConfig) -> Result<(), TuiError> {
    // Set up IPC before touching the terminal so failures print normally.
    let runtime = RuntimeDir::create()?;
    let endpoint = runtime.endpoint();
    let mut server = IpcServer::start(&endpoint)?;
    let mut terminal = ratatui::try_init()?;
    let result = run_loop(&mut terminal, config, &runtime, &endpoint, &mut server).await;
    ratatui::restore();
    result
}

/// Arguments and environment that make `program` report to Sakshi.
///
/// Fails open: if the adapter cannot prepare its hooks, the agent still runs,
/// just without live state.
fn launch_plan(
    program: &str,
    user_args: Vec<String>,
    session_id: SessionId,
    runtime: &RuntimeDir,
    endpoint: &Endpoint,
) -> (AgentKind, Vec<String>, Vec<(String, String)>) {
    let env = vec![
        (SESSION_ENV.to_string(), session_id.to_string()),
        (ENDPOINT_ENV.to_string(), endpoint.to_string()),
    ];
    let Some(adapter) = sakshi_adapters::for_program(program) else {
        return (AgentKind::Generic, user_args, env);
    };
    let prefix = std::env::current_exe()
        .and_then(|hook_exe| {
            adapter.launch_args(&LaunchContext {
                hook_exe: &hook_exe,
                runtime_dir: runtime.path(),
            })
        })
        .unwrap_or_else(|e| {
            tracing::warn!("could not set up {} hooks: {e}", adapter.display_name());
            Vec::new()
        });
    let args = prefix.into_iter().chain(user_args).collect();
    (adapter.kind(), args, env)
}

struct App {
    session: Session,
    pty: PtySession,
    emulator: Box<dyn TerminalEmulator>,
    exit: Option<ExitInfo>,
}

enum Flow {
    Continue,
    Quit,
}

async fn run_loop(
    terminal: &mut DefaultTerminal,
    config: RunConfig,
    runtime: &RuntimeDir,
    endpoint: &Endpoint,
    server: &mut IpcServer,
) -> Result<(), TuiError> {
    let screen = terminal.size()?;
    let size = layout::pty_size(Rect::new(0, 0, screen.width, screen.height));

    let session_id = SessionId::generate();
    let (kind, args, env) =
        launch_plan(&config.program, config.args, session_id, runtime, endpoint);
    let (pty, mut pty_rx) = PtySession::spawn(SpawnConfig {
        program: config.program,
        args,
        cwd: Some(config.cwd.clone()),
        env,
        size,
    })?;
    let mut app = App {
        session: Session::new(session_id, config.name, kind, config.cwd),
        pty,
        emulator: Box::new(Vt100Emulator::new(size)),
        exit: None,
    };

    // crossterm's reader blocks, so it gets its own thread. The thread ends
    // when the process does; it cannot be interrupted while waiting for input.
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    thread::Builder::new()
        .name("sakshi-input".into())
        .spawn(move || {
            while let Ok(ev) = event::read() {
                if event_tx.send(ev).is_err() {
                    break;
                }
            }
        })?;

    let mut tick = tokio::time::interval(Duration::from_millis(100));
    let mut output_open = true;
    let mut ipc_open = true;

    loop {
        terminal.draw(|frame| draw(frame, &app))?;

        tokio::select! {
            ev = event_rx.recv() => match ev {
                Some(ev) => {
                    if let Flow::Quit = app.handle_event(ev) {
                        break;
                    }
                }
                None => break,
            },
            out = pty_rx.recv(), if output_open => match out {
                Some(bytes) => {
                    app.feed(&bytes);
                    // Coalesce whatever else is already queued into one redraw.
                    while let Ok(more) = pty_rx.try_recv() {
                        app.feed(&more);
                    }
                }
                None => output_open = false,
            },
            ev = server.recv(), if ipc_open => match ev {
                Some(ev) => {
                    app.session.apply(&ev);
                }
                None => ipc_open = false,
            },
            _ = tick.tick() => app.poll_exit()?,
        }
    }

    app.pty.kill()?;
    Ok(())
}

impl App {
    /// Feed child output to the emulator and send back any answers it owes.
    fn feed(&mut self, bytes: &[u8]) {
        self.emulator.process(bytes);
        let replies = self.emulator.take_replies();
        if !replies.is_empty() {
            // A failed write means the child is gone; `poll_exit` reports that.
            let _ = self.pty.write(&replies);
        }
    }

    fn handle_event(&mut self, ev: Event) -> Flow {
        match ev {
            Event::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
                self.handle_key(key)
            }
            Event::Resize(width, height) => {
                let size = layout::pty_size(Rect::new(0, 0, width, height));
                self.emulator.resize(size);
                // A failed resize means the child is gone; `poll_exit` reports that.
                let _ = self.pty.resize(size);
                Flow::Continue
            }
            _ => Flow::Continue,
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> Flow {
        let quit_chord =
            key.code == KeyCode::Char('q') && key.modifiers.contains(KeyModifiers::CONTROL);
        if quit_chord || self.exit.is_some() {
            return Flow::Quit;
        }
        if let Some(bytes) = keys::encode_key(key, self.emulator.application_cursor()) {
            // A failed write means the child is gone; `poll_exit` reports that.
            let _ = self.pty.write(&bytes);
        }
        Flow::Continue
    }

    fn poll_exit(&mut self) -> Result<(), TuiError> {
        if self.exit.is_none() {
            if let Some(info) = self.pty.try_wait()? {
                self.session.finish(info.success);
                self.exit = Some(info);
            }
        }
        Ok(())
    }
}

fn draw(frame: &mut Frame, app: &App) {
    let regions = layout::regions(frame.area());

    let session_line = Line::from(format!("{} {}", app.session.state.icon(), app.session.name))
        .style(Style::default().add_modifier(Modifier::REVERSED));
    frame.render_widget(
        Paragraph::new(session_line).block(Block::bordered().title(" Sessions ")),
        regions.sidebar,
    );

    let title = format!(" {} ", app.session.working_dir.display());
    frame.render_widget(Block::bordered().title(title), regions.terminal);
    render_terminal(
        frame.buffer_mut(),
        regions.terminal_inner,
        app.emulator.as_ref(),
    );

    if app.exit.is_none() {
        if let Some((row, col)) = app.emulator.cursor() {
            let inner = regions.terminal_inner;
            if row < inner.height && col < inner.width {
                frame.set_cursor_position((inner.x + col, inner.y + row));
            }
        }
    }

    let hint = match app.exit {
        Some(info) => format!(
            " Process exited with code {} - press any key to close",
            info.code
        ),
        None => " Ctrl+Q quit".to_string(),
    };
    frame.render_widget(
        Paragraph::new(hint).style(Style::default().add_modifier(Modifier::DIM)),
        regions.footer,
    );
}

/// Paint the emulator's screen into `area` of `buf`.
fn render_terminal(buf: &mut Buffer, area: Rect, emulator: &dyn TerminalEmulator) {
    for row in 0..area.height {
        for col in 0..area.width {
            let Some(cell) = emulator.cell(row, col) else {
                continue;
            };
            // The empty right half of a wide character is covered by its left half.
            if cell.symbol.is_empty() {
                continue;
            }
            buf[(area.x + col, area.y + row)]
                .set_symbol(&cell.symbol)
                .set_style(cell.style);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;
    use sakshi_pty::TermSize;

    #[test]
    fn render_terminal_paints_text_and_style() {
        let mut emu = Vt100Emulator::new(TermSize { rows: 3, cols: 10 });
        emu.process(b"\x1b[32mok\x1b[0m");
        let area = Rect::new(2, 1, 10, 3);
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 6));
        render_terminal(&mut buf, area, &emu);
        assert_eq!(buf[(2, 1)].symbol(), "o");
        assert_eq!(buf[(3, 1)].symbol(), "k");
        assert_eq!(buf[(2, 1)].fg, Color::Indexed(2));
        // Outside the pane stays untouched.
        assert_eq!(buf[(0, 0)].symbol(), " ");
    }

    #[test]
    fn render_terminal_tolerates_area_larger_than_screen() {
        let emu = Vt100Emulator::new(TermSize { rows: 2, cols: 4 });
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 10));
        render_terminal(&mut buf, Rect::new(0, 0, 10, 10), &emu);
    }
}
