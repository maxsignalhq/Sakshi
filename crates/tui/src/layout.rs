//! Screen layout: the one place that decides how big the terminal pane is.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::widgets::Block;
use sakshi_pty::TermSize;

/// Width of the session list on the left.
const SIDEBAR_WIDTH: u16 = 26;

/// Width of the details panel on the right, when shown.
const PANEL_WIDTH: u16 = 44;

/// The terminal pane never gets narrower than this because of the panel.
const MIN_TERMINAL_WIDTH: u16 = 30;

/// Screen regions for one frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Regions {
    /// Session list, including its border.
    pub sidebar: Rect,
    /// Terminal pane, including its border.
    pub terminal: Rect,
    /// Interior of the terminal pane, where the child's screen is drawn.
    pub terminal_inner: Rect,
    /// Details panel, if requested and there is room for it.
    pub panel: Option<Rect>,
    /// One-line key hints at the bottom.
    pub footer: Rect,
}

/// Split `area` into regions. The panel is dropped on screens too narrow to
/// show it without squeezing the terminal.
pub fn regions(area: Rect, show_panel: bool) -> Regions {
    let [main, footer] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
    let room_for_panel = main
        .width
        .saturating_sub(SIDEBAR_WIDTH)
        .saturating_sub(PANEL_WIDTH)
        >= MIN_TERMINAL_WIDTH;
    let (sidebar, terminal, panel) = if show_panel && room_for_panel {
        let [sidebar, terminal, panel] = Layout::horizontal([
            Constraint::Length(SIDEBAR_WIDTH),
            Constraint::Min(1),
            Constraint::Length(PANEL_WIDTH),
        ])
        .areas(main);
        (sidebar, terminal, Some(panel))
    } else {
        let [sidebar, terminal] =
            Layout::horizontal([Constraint::Length(SIDEBAR_WIDTH), Constraint::Min(1)]).areas(main);
        (sidebar, terminal, None)
    };
    let terminal_inner = Block::bordered().inner(terminal);
    Regions {
        sidebar,
        terminal,
        terminal_inner,
        panel,
        footer,
    }
}

/// PTY size that fits the terminal pane of a screen of `area`; never below 1x1.
pub fn pty_size(area: Rect, show_panel: bool) -> TermSize {
    let inner = regions(area, show_panel).terminal_inner;
    TermSize {
        rows: inner.height.max(1),
        cols: inner.width.max(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pty_size_accounts_for_sidebar_border_and_footer() {
        let size = pty_size(Rect::new(0, 0, 120, 30), false);
        // width: 120 - 26 sidebar - 2 border; height: 30 - 1 footer - 2 border.
        assert_eq!(size, TermSize { rows: 27, cols: 92 });
    }

    #[test]
    fn the_panel_takes_width_from_the_terminal() {
        let area = Rect::new(0, 0, 140, 30);
        let without = pty_size(area, false);
        let with = pty_size(area, true);
        assert_eq!(without.cols - with.cols, PANEL_WIDTH);
        assert_eq!(without.rows, with.rows);
        let regions = regions(area, true);
        assert_eq!(regions.panel.map(|p| p.width), Some(PANEL_WIDTH));
    }

    #[test]
    fn the_panel_is_dropped_on_narrow_screens() {
        let area = Rect::new(0, 0, 90, 30);
        assert_eq!(regions(area, true).panel, None);
        assert_eq!(pty_size(area, true), pty_size(area, false));
    }

    #[test]
    fn pty_size_never_zero() {
        for show in [false, true] {
            assert_eq!(
                pty_size(Rect::new(0, 0, 1, 1), show),
                TermSize { rows: 1, cols: 1 }
            );
            assert_eq!(
                pty_size(Rect::new(0, 0, 0, 0), show),
                TermSize { rows: 1, cols: 1 }
            );
        }
    }
}
