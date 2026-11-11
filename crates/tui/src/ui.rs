//! Drawing: turns a [`Workspace`] into a frame.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};
use ratatui::Frame;
use sakshi_core::{format_age, Badge, ChangeKind, Severity};
use time::OffsetDateTime;

use crate::emulator::TerminalEmulator;
use crate::layout;
use crate::pane::{GitState, Pane};
use crate::workspace::{Confirm, Focus, Overlay, Workspace};

/// Files listed in the panel's change summary before "and N more".
const MAX_FILES_SHOWN: usize = 8;

/// Activity lines shown in the panel.
const MAX_ACTIVITY_SHOWN: usize = 12;

fn badge_color(badge: Badge) -> Color {
    match badge {
        Badge::Green => Color::Green,
        Badge::Yellow => Color::Yellow,
        Badge::Red => Color::Red,
    }
}

fn badge_label(badge: Badge) -> &'static str {
    match badge {
        Badge::Green => "GREEN",
        Badge::Yellow => "YELLOW",
        Badge::Red => "RED",
    }
}

pub(crate) fn draw(frame: &mut Frame, ws: &Workspace) {
    let regions = layout::regions(frame.area(), ws.show_panel);

    let lines: Vec<Line> = ws
        .panes
        .iter()
        .enumerate()
        .map(|(i, pane)| {
            let label = format!(
                "{} {} {} ",
                i + 1,
                pane.session.state.icon(),
                pane.session.name
            );
            let style = match (i == ws.selected, ws.focus) {
                (true, Focus::List) => Style::default().add_modifier(Modifier::REVERSED),
                (true, Focus::Terminal) => Style::default().add_modifier(Modifier::BOLD),
                _ => Style::default(),
            };
            let badge = Style::default().fg(badge_color(pane.trust.badge));
            Line::from(vec![
                Span::styled(label, style),
                Span::styled("●", style.patch(badge)),
            ])
        })
        .collect();
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(" Sessions ")),
        regions.sidebar,
    );

    match ws.selected_pane() {
        Some(pane) => {
            let scrolled = match pane.scrolled() {
                0 => String::new(),
                rows => format!(" [{rows} rows back; Shift+PgDn returns]"),
            };
            let title = format!(
                " {} - {}{scrolled} ",
                pane.session.name,
                pane.session.working_dir.display()
            );
            frame.render_widget(Block::bordered().title(title), regions.terminal);
            render_terminal(frame.buffer_mut(), regions.terminal_inner, pane.emulator());
            if ws.focus == Focus::Terminal && pane.exit.is_none() && pane.scrolled() == 0 {
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

    if let Some(panel) = regions.panel {
        let attached = ws.focus == Focus::Terminal;
        let lines = match ws.selected_pane() {
            Some(pane) => panel_lines(pane, OffsetDateTime::now_utc(), attached),
            None => vec![Line::from("Nothing selected.")],
        };
        frame.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .block(Block::bordered().title(" Details ")),
            panel,
        );
    }

    if let Some(overlay) = &ws.overlay {
        draw_overlay(frame, overlay, regions.terminal);
    }

    frame.render_widget(
        Paragraph::new(footer_text(ws)).style(Style::default().add_modifier(Modifier::DIM)),
        regions.footer,
    );
}

/// A box centred over `area` holding the "while you were away" summary.
fn draw_overlay(frame: &mut Frame, overlay: &Overlay, area: Rect) {
    let width = area.width.saturating_sub(4).clamp(1, 72);
    let wanted = u16::try_from(overlay.lines.len()).unwrap_or(u16::MAX);
    let height = wanted
        .saturating_add(4)
        .min(area.height.saturating_sub(2).max(1));
    let rect = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    let mut lines: Vec<Line> = overlay.lines.iter().cloned().map(Line::from).collect();
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "Press any key to continue",
        Style::default().add_modifier(Modifier::DIM),
    ));
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(Block::bordered().title(format!(" {} ", overlay.title))),
        rect,
    );
}

fn heading(text: &str) -> Line<'static> {
    Line::styled(
        text.to_string(),
        Style::default().add_modifier(Modifier::BOLD),
    )
}

fn finding_marker(severity: Severity) -> (&'static str, Style) {
    match severity {
        Severity::Alert => ("✗", Style::default().fg(Color::Red)),
        Severity::Warn => ("▲", Style::default().fg(Color::Yellow)),
        Severity::Info => ("·", Style::default().add_modifier(Modifier::DIM)),
    }
}

fn kind_letter(kind: ChangeKind) -> char {
    match kind {
        ChangeKind::Added => 'A',
        ChangeKind::Modified => 'M',
        ChangeKind::Deleted => 'D',
        ChangeKind::Renamed => 'R',
    }
}

/// The details panel for one session: verdict, findings, changes, activity.
///
/// `attached` is whether the user is looking at the session right now. The
/// "since you last looked" digest is only for sessions they are not.
pub(crate) fn panel_lines(pane: &Pane, now: OffsetDateTime, attached: bool) -> Vec<Line<'static>> {
    let badge = pane.trust.badge;
    let mut lines = vec![
        Line::from(vec![
            Span::raw("Trust  "),
            Span::styled(
                badge_label(badge),
                Style::default()
                    .fg(badge_color(badge))
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::raw(""),
        heading("Findings"),
    ];

    if pane.trust.findings.is_empty() {
        lines.push(Line::raw("  Nothing looks off."));
    }
    // Grouped by the question each finding answers, so the panel reads as a
    // few answers rather than a list of rules.
    for (concern, findings) in pane.trust.by_concern() {
        lines.push(Line::styled(
            format!("  {}", concern.title()),
            Style::default().add_modifier(Modifier::DIM),
        ));
        for finding in findings {
            let (marker, style) = finding_marker(finding.severity);
            lines.push(Line::from(vec![
                Span::styled(format!("  {marker} "), style),
                Span::raw(finding.reason.clone()),
            ]));
        }
    }

    let away = pane.away_summary();
    if !attached && !away.is_quiet() {
        let age = format_age((now - pane.last_seen).whole_seconds());
        lines.push(Line::raw(""));
        lines.push(heading(&format!("Since you last looked ({age} ago)")));
        lines.extend(
            away.lines()
                .into_iter()
                .map(|l| Line::raw(format!("  {l}"))),
        );
    }

    lines.push(Line::raw(""));
    lines.push(heading("Changes"));
    match (&pane.git, &pane.diff) {
        (GitState::Pending, _) => lines.push(Line::raw("  Checking repository...")),
        (GitState::NotARepo, _) => lines.push(Line::raw("  Not a git repository.")),
        (GitState::Ready(_), None) => lines.push(Line::raw("  Not checked yet. Press r.")),
        (GitState::Ready(_), Some(diff)) if diff.files.is_empty() => {
            lines.push(Line::raw("  No changes."));
        }
        (GitState::Ready(_), Some(diff)) => {
            lines.push(Line::raw(format!(
                "  {} file(s), +{} -{}",
                diff.files.len(),
                diff.lines_added(),
                diff.lines_removed()
            )));
            for file in diff.files.iter().take(MAX_FILES_SHOWN) {
                lines.push(Line::raw(format!(
                    "  {} {} +{} -{}",
                    kind_letter(file.kind),
                    file.path,
                    file.added.len(),
                    file.removed.len()
                )));
            }
            if diff.files.len() > MAX_FILES_SHOWN {
                lines.push(Line::raw(format!(
                    "  ...and {} more",
                    diff.files.len() - MAX_FILES_SHOWN
                )));
            }
        }
    }

    lines.push(Line::raw(""));
    lines.push(heading("Recent activity"));
    let recent: Vec<Line> = pane
        .events
        .iter()
        .rev()
        .filter_map(|event| {
            let text = event.describe()?;
            let age = format_age((now - event.ts).whole_seconds());
            Some(Line::raw(format!("  {age:>4}  {text}")))
        })
        .take(MAX_ACTIVITY_SHOWN)
        .collect();
    if recent.is_empty() {
        lines.push(Line::raw("  Nothing yet."));
    }
    lines.extend(recent);
    lines
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
            None => " Ctrl+] detach  Shift+PgUp/PgDn scroll".to_string(),
        },
        Focus::List => {
            let mut hints = String::from(" up/down select  Enter attach  n shell");
            for agent in &ws.agents {
                hints.push_str(&format!("  {} {}", agent.key, agent.name));
            }
            hints.push_str("  PgUp/PgDn scroll  t details  r refresh  x close  q quit");
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
    use ratatui::Terminal;
    use sakshi_core::{ChangeKind, Diff, Event, EventKind, FileChange, SessionId, SessionState};
    use sakshi_pty::TermSize;

    use super::*;
    use crate::emulator::Vt100Emulator;
    use crate::workspace::tests::workspace_with;

    fn render_buffer(ws: &Workspace, width: u16, height: u16) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| draw(f, ws)).unwrap();
        terminal.backend().buffer().clone()
    }

    fn render(ws: &Workspace, width: u16, height: u16) -> String {
        let buffer = render_buffer(ws, width, height);
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

    fn text_of(lines: &[Line]) -> String {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn command(id: u64, c: &str) -> Event {
        Event::now(SessionId(id), EventKind::Command { command: c.into() })
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
    fn each_session_shows_a_badge_in_its_trust_colour() {
        let (mut ws, _, _) = workspace_with(&["clean", "risky"]);
        ws.handle_event(&command(2, "git push --force"));
        ws.tick();
        let buffer = render_buffer(&ws, 100, 20);
        // The badge is the last cell of each list row, inside the border.
        let dot = |y: u16| {
            (1..26u16)
                .map(|x| &buffer[(x, y)])
                .find(|c| c.symbol() == "●")
                .map(|c| c.fg)
        };
        assert_eq!(dot(1), Some(Color::Green));
        assert_eq!(dot(2), Some(Color::Red));
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
    fn the_details_panel_appears_when_toggled_and_there_is_room() {
        let (mut ws, _, _) = workspace_with(&["alpha"]);
        assert!(!render(&ws, 160, 30).contains("Details"));
        ws.handle_key(press(KeyCode::Char('t')));
        let screen = render(&ws, 160, 30);
        assert!(screen.contains("Details"), "{screen}");
        assert!(screen.contains("Trust  GREEN"), "{screen}");
        // Too narrow for the panel: it is quietly left out.
        assert!(!render(&ws, 90, 30).contains("Details"));
    }

    #[test]
    fn the_panel_shows_findings_changes_and_activity() {
        let (mut ws, _, _) = workspace_with(&["alpha"]);
        ws.handle_event(&command(1, "rm -rf /"));
        ws.tick();
        ws.panes[0].git = crate::pane::GitState::NotARepo;
        let lines = panel_lines(&ws.panes[0], OffsetDateTime::now_utc(), false);
        let text = text_of(&lines);
        assert!(text.contains("Trust  RED"), "{text}");
        assert!(text.contains("  Risky actions\n"), "{text}");
        assert!(text.contains("✗ ran `rm -rf /`"), "{text}");
        assert!(text.contains("Not a git repository."), "{text}");
        assert!(text.contains("$ rm -rf /"), "{text}");
    }

    #[test]
    fn a_clean_session_says_so() {
        let (ws, _, _) = workspace_with(&["alpha"]);
        let text = text_of(&panel_lines(&ws.panes[0], OffsetDateTime::now_utc(), false));
        assert!(text.contains("Nothing looks off."), "{text}");
        assert!(text.contains("Checking repository..."), "{text}");
        assert!(text.contains("Nothing yet."), "{text}");
    }

    #[test]
    fn the_change_summary_lists_files_and_caps_the_list() {
        let (mut ws, _, _) = workspace_with(&["alpha"]);
        let files = (0..12)
            .map(|i| {
                let mut f = FileChange::new(format!("src/f{i}.rs"), ChangeKind::Modified);
                f.added = vec!["x".into(); 3];
                f.removed = vec!["y".into()];
                f
            })
            .collect();
        ws.panes[0].set_diff(Diff { files });
        ws.panes[0].git = crate::pane::GitState::NotARepo; // ignored once a diff exists? no
                                                           // With a ready repository the summary is shown; emulate with a fake.
        struct Fake;
        impl crate::pane::DiffSource for Fake {
            fn diff(&self) -> std::io::Result<Diff> {
                Ok(Diff::default())
            }
        }
        ws.panes[0].git = crate::pane::GitState::Ready(std::sync::Arc::new(Fake));
        let text = text_of(&panel_lines(&ws.panes[0], OffsetDateTime::now_utc(), false));
        assert!(text.contains("12 file(s), +36 -12"), "{text}");
        assert!(text.contains("M src/f0.rs +3 -1"), "{text}");
        assert!(text.contains("...and 4 more"), "{text}");
        assert!(!text.contains("src/f8.rs"), "{text}");
    }

    #[test]
    fn recent_activity_is_newest_first_with_ages() {
        let (mut ws, _, _) = workspace_with(&["alpha"]);
        let now = OffsetDateTime::now_utc();
        for (secs_ago, text) in [(120, "older"), (5, "newer")] {
            ws.handle_event(&Event {
                session_id: SessionId(1),
                ts: now - time::Duration::seconds(secs_ago),
                kind: EventKind::Message { text: text.into() },
            });
        }
        let text = text_of(&panel_lines(&ws.panes[0], now, false));
        let newer = text.find("newer").unwrap();
        let older = text.find("older").unwrap();
        assert!(newer < older, "{text}");
        assert!(text.contains("2m  older"), "{text}");
        assert!(text.contains("5s  newer"), "{text}");
    }

    #[test]
    fn the_panel_digests_what_happened_since_you_last_looked() {
        let (mut ws, _, _) = workspace_with(&["alpha"]);
        let now = OffsetDateTime::now_utc();
        ws.panes[0].last_seen = now - time::Duration::seconds(90);
        ws.handle_event(&Event {
            session_id: SessionId(1),
            ts: now - time::Duration::seconds(30),
            kind: EventKind::FileEdit {
                path: "src/a.rs".into(),
            },
        });
        let text = text_of(&panel_lines(&ws.panes[0], now, false));
        assert!(text.contains("Since you last looked (1m ago)"), "{text}");
        assert!(text.contains("Edited 1 file(s): src/a.rs"), "{text}");
        // Not shown for the session being looked at right now.
        let attached = text_of(&panel_lines(&ws.panes[0], now, true));
        assert!(!attached.contains("Since you last looked"), "{attached}");
    }

    #[test]
    fn the_overlay_is_drawn_over_the_terminal_until_a_key() {
        let (mut ws, _, _) = workspace_with(&["alpha"]);
        ws.panes[0].last_seen = OffsetDateTime::now_utc() - time::Duration::seconds(60);
        ws.handle_event(&Event {
            session_id: SessionId(1),
            ts: OffsetDateTime::now_utc() - time::Duration::seconds(30),
            kind: EventKind::FileEdit {
                path: "src/a.rs".into(),
            },
        });
        ws.handle_key(press(KeyCode::Enter));
        let screen = render(&ws, 120, 30);
        assert!(screen.contains("while you were away"), "{screen}");
        assert!(screen.contains("Edited 1 file(s): src/a.rs"), "{screen}");
        assert!(screen.contains("Press any key to continue"), "{screen}");
        ws.handle_key(press(KeyCode::Char('x')));
        assert!(!render(&ws, 120, 30).contains("while you were away"));
    }

    #[test]
    fn a_scrolled_view_says_so_in_the_title_and_hides_the_cursor() {
        let (mut ws, _, _) = workspace_with(&["alpha"]);
        let text: String = (0..300).map(|i| format!("row{i}\r\n")).collect();
        ws.handle_output(SessionId(1), text.as_bytes());
        ws.handle_key(press(KeyCode::Enter));
        assert!(!render(&ws, 120, 30).contains("rows back"));
        ws.handle_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::SHIFT));
        let screen = render(&ws, 120, 30);
        assert!(screen.contains("rows back"), "{screen}");
        // The older output is on screen, not the live tail.
        assert!(!screen.contains("row299"), "{screen}");
    }

    #[test]
    fn footer_changes_with_focus_and_confirmation() {
        let (mut ws, _, _) = workspace_with(&["alpha"]);
        assert!(footer_text(&ws).contains("Enter attach"));
        assert!(footer_text(&ws).contains("c Claude Code"));
        assert!(footer_text(&ws).contains("t details"));
        ws.handle_key(press(KeyCode::Char('x')));
        assert!(footer_text(&ws).contains("x again"));
        ws.handle_key(press(KeyCode::Char('j')));
        ws.handle_key(press(KeyCode::Enter));
        assert!(footer_text(&ws).contains("Ctrl+] detach"));
        ws.notice = Some("could not start".into());
        assert!(footer_text(&ws).contains("could not start"));
    }
}
