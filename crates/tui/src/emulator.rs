//! Terminal emulation behind a trait so the `vt100` crate can be swapped out.

use std::num::NonZeroU16;

use ratatui::style::{Color, Modifier, Style};
use sakshi_pty::TermSize;

/// One rendered screen cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellView {
    /// Text of the cell; empty for the blank right half of a wide character.
    pub symbol: String,
    /// Colors and attributes.
    pub style: Style,
}

/// A terminal emulator: consumes child output, exposes the resulting screen.
pub trait TerminalEmulator {
    /// Feed raw output bytes from the child.
    fn process(&mut self, bytes: &[u8]);
    /// Resize the emulated screen.
    fn resize(&mut self, size: TermSize);
    /// Current screen size.
    fn size(&self) -> TermSize;
    /// The cell at `row`, `col`, or `None` when out of bounds.
    fn cell(&self, row: u16, col: u16) -> Option<CellView>;
    /// Cursor position `(row, col)`, or `None` when the child hid the cursor.
    fn cursor(&self) -> Option<(u16, u16)>;
    /// Whether the child asked for application cursor keys (DECCKM).
    fn application_cursor(&self) -> bool;
    /// Take the bytes the emulator owes the child as answers to its queries
    /// (for example the cursor position report), leaving none pending.
    fn take_replies(&mut self) -> Vec<u8>;
}

fn nz(v: u16) -> NonZeroU16 {
    NonZeroU16::new(v).unwrap_or(NonZeroU16::MIN)
}

/// Collects answers to terminal queries that `vt100` parses but does not answer.
///
/// Windows ConPTY asks for the cursor position at startup and holds the child
/// until it gets a reply, so an emulator that stays silent hangs every session.
#[derive(Default)]
struct Replies {
    pending: Vec<u8>,
}

impl vt100::Callbacks for Replies {
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        // DSR 6: cursor position report, `CSI 6 n` -> `CSI row ; col R`.
        let is_cursor_query = c == 'n' && i1.is_none() && i2.is_none() && matches!(params, [[6]]);
        if is_cursor_query {
            let (row, col) = screen.cursor_position();
            self.pending
                .extend(format!("\x1b[{};{}R", row + 1, col + 1).into_bytes());
        }
    }
}

/// [`TerminalEmulator`] backed by the `vt100` crate.
pub struct Vt100Emulator {
    parser: vt100::Parser<Replies>,
}

impl Vt100Emulator {
    /// Create an emulator of the given size with a modest scrollback.
    pub fn new(size: TermSize) -> Self {
        Self {
            parser: vt100::Parser::new_with_callbacks(
                nz(size.rows),
                nz(size.cols),
                1000,
                Replies::default(),
            ),
        }
    }
}

fn convert_color(c: vt100::Color) -> Color {
    match c {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

impl TerminalEmulator for Vt100Emulator {
    fn process(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
    }

    fn resize(&mut self, size: TermSize) {
        self.parser.set_size(nz(size.rows), nz(size.cols));
    }

    fn size(&self) -> TermSize {
        let (rows, cols) = self.parser.screen().size();
        TermSize {
            rows: rows.get(),
            cols: cols.get(),
        }
    }

    fn cell(&self, row: u16, col: u16) -> Option<CellView> {
        let cell = self.parser.screen().cell(row, col)?;
        let mut style = Style::default()
            .fg(convert_color(cell.fgcolor()))
            .bg(convert_color(cell.bgcolor()));
        for (on, modifier) in [
            (cell.bold(), Modifier::BOLD),
            (cell.dim(), Modifier::DIM),
            (cell.italic(), Modifier::ITALIC),
            (cell.underline(), Modifier::UNDERLINED),
            (cell.inverse(), Modifier::REVERSED),
        ] {
            if on {
                style = style.add_modifier(modifier);
            }
        }
        let symbol = if cell.is_wide_continuation() {
            String::new()
        } else if cell.has_contents() {
            cell.contents().to_string()
        } else {
            " ".to_string()
        };
        Some(CellView { symbol, style })
    }

    fn cursor(&self) -> Option<(u16, u16)> {
        let screen = self.parser.screen();
        if screen.hide_cursor() {
            None
        } else {
            Some(screen.cursor_position())
        }
    }

    fn application_cursor(&self) -> bool {
        self.parser.screen().application_cursor()
    }

    fn take_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.parser.callbacks_mut().pending)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn emu() -> Vt100Emulator {
        Vt100Emulator::new(TermSize { rows: 5, cols: 20 })
    }

    fn symbol(e: &Vt100Emulator, row: u16, col: u16) -> String {
        e.cell(row, col).map(|c| c.symbol).unwrap_or_default()
    }

    #[test]
    fn renders_text_and_newlines() {
        let mut e = emu();
        e.process(b"hi\r\nyo");
        assert_eq!(symbol(&e, 0, 0), "h");
        assert_eq!(symbol(&e, 0, 1), "i");
        assert_eq!(symbol(&e, 1, 1), "o");
        assert_eq!(symbol(&e, 3, 3), " ");
    }

    #[test]
    fn applies_colors_and_attributes() {
        let mut e = emu();
        e.process(b"\x1b[31;1mR\x1b[0m");
        let cell = e.cell(0, 0).unwrap();
        assert_eq!(cell.style.fg, Some(Color::Indexed(1)));
        assert!(cell.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn tracks_cursor_and_visibility() {
        let mut e = emu();
        e.process(b"abc");
        assert_eq!(e.cursor(), Some((0, 3)));
        e.process(b"\x1b[?25l");
        assert_eq!(e.cursor(), None);
    }

    #[test]
    fn resize_changes_size_and_clamps_zero() {
        let mut e = emu();
        e.resize(TermSize { rows: 10, cols: 40 });
        assert_eq!(e.size(), TermSize { rows: 10, cols: 40 });
        e.resize(TermSize { rows: 0, cols: 0 });
        assert_eq!(e.size(), TermSize { rows: 1, cols: 1 });
    }

    #[test]
    fn out_of_bounds_cell_is_none() {
        assert!(emu().cell(99, 99).is_none());
    }

    #[test]
    fn answers_cursor_position_query() {
        let mut e = emu();
        e.process(b"\x1b[6n");
        assert_eq!(e.take_replies(), b"\x1b[1;1R".to_vec());
        assert!(e.take_replies().is_empty());
        e.process(b"ab\r\nc\x1b[6n");
        assert_eq!(e.take_replies(), b"\x1b[2;2R".to_vec());
    }

    #[test]
    fn reports_application_cursor_mode() {
        let mut e = emu();
        assert!(!e.application_cursor());
        e.process(b"\x1b[?1h");
        assert!(e.application_cursor());
    }
}
