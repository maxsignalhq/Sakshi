//! Drawing: turns a [`Workspace`] into a frame.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use crate::emulator::TerminalEmulator;
use crate::layout;
use crate::workspace::{Confirm, Focus, Workspace};

pub(crate) fn draw(frame: &mut Frame, ws: &Workspace) {
    let regions = layout::regions(frame.area());

    let lines: Vec<Line> = ws
        .panes
        .iter()
        .enumerate()
        .map(|(i, pane)| {
            let label = format!(
                "{} {} {}",
                i + 1,
                pane.session.state.icon(),
                pane.session.name
            );
            let style = match (i == ws.selected, ws.focus) {
                (true, Focus::List) => Style::default().add_modifier(Modifier::REVERSED),
                (true, Focus::Terminal) => Style::default().add_modifier(Modifier::BOLD),
                _ => Style::default(),
            };
            Line::styled(label, style)
        })
        .collect();
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(" Sessions ")),
        regions.sidebar,
    );

    match ws.selected_pane() {
        Some(pane) => {
            let title = format!(
                " {} - {} ",
                pane.session.name,
                pane.session.working_dir.display()
            );
            frame.render_widget(Block::bordered().title(title), regions.terminal);
            render_terminal(frame.buffer_mut(), regions.terminal_inner, pane.emulator());
            if ws.focus == Focus::Terminal && pane.exit.is_none() {
                if let Some((row, col)) = pane.emulator().cursor() {
                    let inner = regions.terminal_inner;
                    if row < inner.height && col < inner.width {
                        frame.set_cursor_position((inner.x + col, inner.y + row));
                    }
                }
            }
        }
        None => {
            frame.render_widget(
                Paragraph::new("No sessions. Press n to start a shell.")
                    .block(Block::bordered().title(" Sakshi ")),
                regions.terminal,
            );
        }
    }

    frame.render_widget(
        Paragraph::new(footer_text(ws)).style(Style::default().add_modifier(Modifier::DIM)),
        regions.footer,
    );
}

/// The key hints or message shown in the footer.
pub(crate) fn footer_text(ws: &Workspace) -> String {
    if let Some(notice) = &ws.notice {
        return format!(" {notice}");
    }
    match ws.confirm {
        Some(Confirm::Close(_)) => {
            return " Close this session and kill its process? Press x again to confirm".into();
        }
        Some(Confirm::Quit) => {
            return " Sessions are still running. Press q again to quit and kill them".into();
        }
        None => {}
    }
    match ws.focus {
        Focus::Terminal => match ws.selected_pane().and_then(|p| p.exit) {
            Some(info) => format!(
                " Process exited with code {} - press any key to return to the list",
                info.code
            ),
            None => " Ctrl+] detach".to_string(),
        },
        Focus::List => {
            let mut hints = String::from(" up/down select  Enter attach  n shell");
            for agent in &ws.agents {
                hints.push_str(&format!("  {} {}", agent.key, agent.name));
            }
            hints.push_str("  x close  q quit");
            hints
        }
    }
}

/// Paint the emulator's screen into `area` of `buf`.
pub(crate) fn render_terminal(buf: &mut Buffer, area: Rect, emulator: &dyn TerminalEmulator) {
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
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::style::Color;
    use ratatui::Terminal;
    use sakshi_core::{Event, EventKind, SessionId, SessionState};
    use sakshi_pty::TermSize;

    use super::*;
    use crate::emulator::Vt100Emulator;
    use crate::workspace::tests::workspace_with;

    fn render(ws: &Workspace, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| draw(f, ws)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

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
        assert_eq!(buf[(0, 0)].symbol(), " ");
    }

    #[test]
    fn render_terminal_tolerates_area_larger_than_screen() {
        let emu = Vt100Emulator::new(TermSize { rows: 2, cols: 4 });
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 10));
        render_terminal(&mut buf, Rect::new(0, 0, 10, 10), &emu);
    }

    #[test]
    fn the_session_list_shows_numbers_icons_and_names() {
        let (mut ws, _, _) = workspace_with(&["alpha", "beta"]);
        ws.handle_event(&Event::now(
            SessionId(2),
            EventKind::State(SessionState::Waiting),
        ));
        let screen = render(&ws, 100, 20);
        assert!(screen.contains("1 ○ alpha"), "{screen}");
        assert!(screen.contains("2 ! beta"), "{screen}");
    }

    #[test]
    fn an_empty_workspace_explains_what_to_do() {
        let (ws, _, _) = workspace_with(&[]);
        let screen = render(&ws, 100, 20);
        assert!(screen.contains("No sessions"), "{screen}");
    }

    #[test]
    fn the_selected_session_screen_is_drawn() {
        let (mut ws, _, _) = workspace_with(&["alpha", "beta"]);
        ws.handle_output(SessionId(1), b"from-alpha");
        ws.handle_output(SessionId(2), b"from-beta");
        assert!(render(&ws, 100, 20).contains("from-alpha"));
        ws.handle_key(press(KeyCode::Down));
        let screen = render(&ws, 100, 20);
        assert!(screen.contains("from-beta"), "{screen}");
        assert!(!screen.contains("from-alpha"), "{screen}");
    }

    #[test]
    fn footer_changes_with_focus_and_confirmation() {
        let (mut ws, _, _) = workspace_with(&["alpha"]);
        assert!(footer_text(&ws).contains("Enter attach"));
        assert!(footer_text(&ws).contains("c Claude Code"));
        ws.handle_key(press(KeyCode::Char('x')));
        assert!(footer_text(&ws).contains("press x again") || footer_text(&ws).contains("x again"));
        ws.handle_key(press(KeyCode::Char('j')));
        ws.handle_key(press(KeyCode::Enter));
        assert!(footer_text(&ws).contains("Ctrl+] detach"));
        ws.notice = Some("could not start".into());
        assert!(footer_text(&ws).contains("could not start"));
    }
}
