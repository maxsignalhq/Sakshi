//! Screen layout: the one place that decides how big the terminal pane is.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::widgets::Block;
use sakshi_pty::TermSize;

/// Width of the session list on the left.
const SIDEBAR_WIDTH: u16 = 24;

/// Screen regions for one frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Regions {
    /// Session list, including its border.
    pub sidebar: Rect,
    /// Terminal pane, including its border.
    pub terminal: Rect,
    /// Interior of the terminal pane, where the child's screen is drawn.
    pub terminal_inner: Rect,
    /// One-line key hints at the bottom.
    pub footer: Rect,
}

/// Split `area` into regions.
pub fn regions(area: Rect) -> Regions {
    let [main, footer] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
    let [sidebar, terminal] =
        Layout::horizontal([Constraint::Length(SIDEBAR_WIDTH), Constraint::Min(1)]).areas(main);
    let terminal_inner = Block::bordered().inner(terminal);
    Regions {
        sidebar,
        terminal,
        terminal_inner,
        footer,
    }
}

/// PTY size that fits the terminal pane of a screen of `area`; never below 1x1.
pub fn pty_size(area: Rect) -> TermSize {
    let inner = regions(area).terminal_inner;
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
        let size = pty_size(Rect::new(0, 0, 100, 30));
        // width: 100 - 24 sidebar - 2 border; height: 30 - 1 footer - 2 border.
        assert_eq!(size, TermSize { rows: 27, cols: 74 });
    }

    #[test]
    fn pty_size_never_zero() {
        assert_eq!(
            pty_size(Rect::new(0, 0, 1, 1)),
            TermSize { rows: 1, cols: 1 }
        );
        assert_eq!(
            pty_size(Rect::new(0, 0, 0, 0)),
            TermSize { rows: 1, cols: 1 }
        );
    }
}
