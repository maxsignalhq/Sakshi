//! Translate key events and pastes into the bytes a terminal program expects
//! on stdin.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Start of a bracketed paste.
const PASTE_START: &str = "\x1b[200~";
/// End of a bracketed paste.
const PASTE_END: &str = "\x1b[201~";

/// The xterm modifier parameter: 1 plus shift 1, alt 2, ctrl 4.
fn modifier_param(mods: KeyModifiers) -> u8 {
    1 + u8::from(mods.contains(KeyModifiers::SHIFT))
        + 2 * u8::from(mods.contains(KeyModifiers::ALT))
        + 4 * u8::from(mods.contains(KeyModifiers::CONTROL))
}

/// Encode `key` for the child, or `None` for keys with no terminal encoding.
///
/// `app_cursor` selects the application-mode (`ESC O`) arrow/Home/End
/// sequences for unmodified keys. Modified cursor, editing and function keys
/// use the xterm `CSI 1 ; <modifier> <key>` forms, so Ctrl+Left jumps a word
/// in a shell and Shift+Tab, Alt+Right and friends reach the program.
pub fn encode_key(key: KeyEvent, app_cursor: bool) -> Option<Vec<u8>> {
    let mods = key.modifiers;
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    let alt = mods.contains(KeyModifiers::ALT);
    let m = modifier_param(mods);

    let mut bytes: Vec<u8> = match key.code {
        KeyCode::Char(c) if ctrl => vec![control_byte(c)?],
        KeyCode::Char(c) => c.to_string().into_bytes(),
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Esc => vec![0x1b],
        // Keys with their own modified forms never get the extra Alt prefix.
        KeyCode::Up => return Some(cursor_key(b'A', app_cursor, m)),
        KeyCode::Down => return Some(cursor_key(b'B', app_cursor, m)),
        KeyCode::Right => return Some(cursor_key(b'C', app_cursor, m)),
        KeyCode::Left => return Some(cursor_key(b'D', app_cursor, m)),
        KeyCode::Home => return Some(cursor_key(b'H', app_cursor, m)),
        KeyCode::End => return Some(cursor_key(b'F', app_cursor, m)),
        KeyCode::PageUp => return Some(tilde_key(5, m)),
        KeyCode::PageDown => return Some(tilde_key(6, m)),
        KeyCode::Insert => return Some(tilde_key(2, m)),
        KeyCode::Delete => return Some(tilde_key(3, m)),
        KeyCode::F(n) => return function_key(n, m),
        _ => return None,
    };

    if alt {
        bytes.insert(0, 0x1b);
    }
    Some(bytes)
}

fn control_byte(c: char) -> Option<u8> {
    match c {
        ' ' | '@' => Some(0),
        'a'..='z' => Some(c as u8 - b'a' + 1),
        'A'..='Z' => Some(c as u8 - b'A' + 1),
        '['..='_' => Some(c as u8 - b'@'),
        _ => None,
    }
}

/// Arrows, Home and End: `ESC [ X`, `ESC O X` in application mode, or
/// `ESC [ 1 ; m X` with modifiers.
fn cursor_key(final_byte: u8, app_cursor: bool, m: u8) -> Vec<u8> {
    let mut v = if m > 1 {
        format!("\x1b[1;{m}").into_bytes()
    } else if app_cursor {
        b"\x1bO".to_vec()
    } else {
        b"\x1b[".to_vec()
    };
    v.push(final_byte);
    v
}

/// PageUp, Insert and friends: `ESC [ n ~`, or `ESC [ n ; m ~` with modifiers.
fn tilde_key(n: u8, m: u8) -> Vec<u8> {
    if m > 1 {
        format!("\x1b[{n};{m}~").into_bytes()
    } else {
        format!("\x1b[{n}~").into_bytes()
    }
}

fn function_key(n: u8, m: u8) -> Option<Vec<u8>> {
    // F1-F4 are `ESC O P..S` plain and `ESC [ 1 ; m P..S` modified.
    if let 1..=4 = n {
        let final_byte = b'P' + (n - 1);
        return Some(if m > 1 {
            format!("\x1b[1;{m}{}", final_byte as char).into_bytes()
        } else {
            vec![0x1b, b'O', final_byte]
        });
    }
    let code = match n {
        5 => 15,
        6 => 17,
        7 => 18,
        8 => 19,
        9 => 20,
        10 => 21,
        11 => 23,
        12 => 24,
        _ => return None,
    };
    Some(tilde_key(code, m))
}

/// Bytes to send for pasted `text`.
///
/// With `bracketed`, the program asked to be told about pastes: the text is
/// wrapped in the paste markers so it can treat newlines as text rather than
/// as Enter. Without it, line breaks become the Enter a keyboard would send.
/// Escape characters are dropped either way, so a paste cannot smuggle in
/// terminal control sequences, least of all the end marker.
pub fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let clean: String = text.chars().filter(|&c| c != '\x1b').collect();
    if bracketed {
        format!("{PASTE_START}{clean}{PASTE_END}").into_bytes()
    } else {
        clean.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enc(code: KeyCode, mods: KeyModifiers, app: bool) -> Option<Vec<u8>> {
        encode_key(KeyEvent::new(code, mods), app)
    }

    fn plain(code: KeyCode) -> Option<Vec<u8>> {
        enc(code, KeyModifiers::NONE, false)
    }

    #[test]
    fn plain_and_unicode_chars() {
        assert_eq!(plain(KeyCode::Char('a')), Some(b"a".to_vec()));
        assert_eq!(plain(KeyCode::Char('é')), Some("é".as_bytes().to_vec()));
        // Shift is already in the character, so it adds nothing.
        assert_eq!(
            enc(KeyCode::Char('A'), KeyModifiers::SHIFT, false),
            Some(b"A".to_vec())
        );
    }

    #[test]
    fn control_chars() {
        let ctrl = |c| enc(KeyCode::Char(c), KeyModifiers::CONTROL, false);
        assert_eq!(ctrl('c'), Some(vec![3]));
        assert_eq!(ctrl('d'), Some(vec![4]));
        assert_eq!(ctrl('['), Some(vec![0x1b]));
        assert_eq!(ctrl('1'), None);
    }

    #[test]
    fn alt_prefixes_escape() {
        assert_eq!(
            enc(KeyCode::Char('b'), KeyModifiers::ALT, false),
            Some(vec![0x1b, b'b'])
        );
        assert_eq!(
            enc(KeyCode::Enter, KeyModifiers::ALT, false),
            Some(vec![0x1b, b'\r'])
        );
    }

    #[test]
    fn arrows_follow_application_mode_when_unmodified() {
        assert_eq!(plain(KeyCode::Up), Some(b"\x1b[A".to_vec()));
        assert_eq!(
            enc(KeyCode::Up, KeyModifiers::NONE, true),
            Some(b"\x1bOA".to_vec())
        );
    }

    #[test]
    fn modified_cursor_keys_use_the_xterm_forms() {
        // Ctrl+Left / Ctrl+Right jump words in most shells and editors.
        for (mods, m) in [
            (KeyModifiers::SHIFT, 2),
            (KeyModifiers::ALT, 3),
            (KeyModifiers::CONTROL, 5),
            (KeyModifiers::CONTROL | KeyModifiers::SHIFT, 6),
            (KeyModifiers::CONTROL | KeyModifiers::ALT, 7),
        ] {
            assert_eq!(
                enc(KeyCode::Left, mods, false),
                Some(format!("\x1b[1;{m}D").into_bytes()),
                "{mods:?}"
            );
            // Application mode does not change the modified form.
            assert_eq!(
                enc(KeyCode::Right, mods, true),
                Some(format!("\x1b[1;{m}C").into_bytes()),
                "{mods:?}"
            );
        }
        assert_eq!(
            enc(KeyCode::Home, KeyModifiers::CONTROL, false),
            Some(b"\x1b[1;5H".to_vec())
        );
        assert_eq!(
            enc(KeyCode::End, KeyModifiers::SHIFT, false),
            Some(b"\x1b[1;2F".to_vec())
        );
    }

    #[test]
    fn modified_editing_keys_use_the_tilde_forms() {
        assert_eq!(
            enc(KeyCode::Delete, KeyModifiers::CONTROL, false),
            Some(b"\x1b[3;5~".to_vec())
        );
        assert_eq!(
            enc(KeyCode::PageUp, KeyModifiers::ALT, false),
            Some(b"\x1b[5;3~".to_vec())
        );
        assert_eq!(plain(KeyCode::Delete), Some(b"\x1b[3~".to_vec()));
        assert_eq!(plain(KeyCode::Insert), Some(b"\x1b[2~".to_vec()));
        assert_eq!(plain(KeyCode::PageDown), Some(b"\x1b[6~".to_vec()));
    }

    #[test]
    fn editing_and_function_keys() {
        assert_eq!(plain(KeyCode::Enter), Some(vec![b'\r']));
        assert_eq!(plain(KeyCode::Backspace), Some(vec![0x7f]));
        assert_eq!(plain(KeyCode::BackTab), Some(b"\x1b[Z".to_vec()));
        assert_eq!(plain(KeyCode::F(1)), Some(b"\x1bOP".to_vec()));
        assert_eq!(plain(KeyCode::F(4)), Some(b"\x1bOS".to_vec()));
        assert_eq!(plain(KeyCode::F(5)), Some(b"\x1b[15~".to_vec()));
        assert_eq!(plain(KeyCode::F(12)), Some(b"\x1b[24~".to_vec()));
        assert_eq!(plain(KeyCode::F(13)), None);
        assert_eq!(
            enc(KeyCode::F(1), KeyModifiers::SHIFT, false),
            Some(b"\x1b[1;2P".to_vec())
        );
        assert_eq!(
            enc(KeyCode::F(5), KeyModifiers::CONTROL, false),
            Some(b"\x1b[15;5~".to_vec())
        );
    }

    #[test]
    fn bracketed_paste_wraps_the_text() {
        assert_eq!(
            paste_bytes("line one\nline two", true),
            b"\x1b[200~line one\nline two\x1b[201~".to_vec()
        );
    }

    #[test]
    fn plain_paste_turns_line_breaks_into_enter() {
        assert_eq!(paste_bytes("a\nb\r\nc", false), b"a\rb\rc".to_vec());
        assert_eq!(paste_bytes("", false), Vec::<u8>::new());
    }

    #[test]
    fn a_paste_cannot_carry_escape_sequences() {
        let evil = "ok\x1b[201~rm -rf /\x1b]0;title\x07";
        for bracketed in [true, false] {
            let out = String::from_utf8(paste_bytes(evil, bracketed)).unwrap();
            let body = out
                .strip_prefix(PASTE_START)
                .and_then(|b| b.strip_suffix(PASTE_END))
                .unwrap_or(&out);
            assert!(!body.contains('\x1b'), "{bracketed}: {out:?}");
        }
    }
}
