//! Translate key events into the bytes a terminal program expects on stdin.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Encode `key` for the child, or `None` for keys with no terminal encoding.
///
/// `app_cursor` selects the application-mode (`ESC O`) arrow/Home/End sequences.
pub fn encode_key(key: KeyEvent, app_cursor: bool) -> Option<Vec<u8>> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);

    let mut bytes: Vec<u8> = match key.code {
        KeyCode::Char(c) if ctrl => vec![control_byte(c)?],
        KeyCode::Char(c) => c.to_string().into_bytes(),
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => cursor_key(b'A', app_cursor),
        KeyCode::Down => cursor_key(b'B', app_cursor),
        KeyCode::Right => cursor_key(b'C', app_cursor),
        KeyCode::Left => cursor_key(b'D', app_cursor),
        KeyCode::Home => cursor_key(b'H', app_cursor),
        KeyCode::End => cursor_key(b'F', app_cursor),
        KeyCode::PageUp => b"\x1b[5~".to_vec(),
        KeyCode::PageDown => b"\x1b[6~".to_vec(),
        KeyCode::Insert => b"\x1b[2~".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        KeyCode::F(n) => function_key(n)?,
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

fn cursor_key(final_byte: u8, app_cursor: bool) -> Vec<u8> {
    let intro: &[u8] = if app_cursor { b"\x1bO" } else { b"\x1b[" };
    let mut v = intro.to_vec();
    v.push(final_byte);
    v
}

fn function_key(n: u8) -> Option<Vec<u8>> {
    let seq: &[u8] = match n {
        1 => b"\x1bOP",
        2 => b"\x1bOQ",
        3 => b"\x1bOR",
        4 => b"\x1bOS",
        5 => b"\x1b[15~",
        6 => b"\x1b[17~",
        7 => b"\x1b[18~",
        8 => b"\x1b[19~",
        9 => b"\x1b[20~",
        10 => b"\x1b[21~",
        11 => b"\x1b[23~",
        12 => b"\x1b[24~",
        _ => return None,
    };
    Some(seq.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enc(code: KeyCode, mods: KeyModifiers, app: bool) -> Option<Vec<u8>> {
        encode_key(KeyEvent::new(code, mods), app)
    }

    #[test]
    fn plain_and_unicode_chars() {
        assert_eq!(
            enc(KeyCode::Char('a'), KeyModifiers::NONE, false),
            Some(b"a".to_vec())
        );
        assert_eq!(
            enc(KeyCode::Char('é'), KeyModifiers::NONE, false),
            Some("é".as_bytes().to_vec())
        );
    }

    #[test]
    fn control_chars() {
        assert_eq!(
            enc(KeyCode::Char('c'), KeyModifiers::CONTROL, false),
            Some(vec![3])
        );
        assert_eq!(
            enc(KeyCode::Char('d'), KeyModifiers::CONTROL, false),
            Some(vec![4])
        );
        assert_eq!(
            enc(KeyCode::Char('['), KeyModifiers::CONTROL, false),
            Some(vec![0x1b])
        );
        assert_eq!(enc(KeyCode::Char('1'), KeyModifiers::CONTROL, false), None);
    }

    #[test]
    fn alt_prefixes_escape() {
        assert_eq!(
            enc(KeyCode::Char('b'), KeyModifiers::ALT, false),
            Some(vec![0x1b, b'b'])
        );
    }

    #[test]
    fn arrows_follow_application_mode() {
        assert_eq!(
            enc(KeyCode::Up, KeyModifiers::NONE, false),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            enc(KeyCode::Up, KeyModifiers::NONE, true),
            Some(b"\x1bOA".to_vec())
        );
    }

    #[test]
    fn editing_and_function_keys() {
        assert_eq!(
            enc(KeyCode::Enter, KeyModifiers::NONE, false),
            Some(vec![b'\r'])
        );
        assert_eq!(
            enc(KeyCode::Backspace, KeyModifiers::NONE, false),
            Some(vec![0x7f])
        );
        assert_eq!(
            enc(KeyCode::Delete, KeyModifiers::NONE, false),
            Some(b"\x1b[3~".to_vec())
        );
        assert_eq!(
            enc(KeyCode::F(1), KeyModifiers::NONE, false),
            Some(b"\x1bOP".to_vec())
        );
        assert_eq!(
            enc(KeyCode::F(12), KeyModifiers::NONE, false),
            Some(b"\x1b[24~".to_vec())
        );
        assert_eq!(enc(KeyCode::F(13), KeyModifiers::NONE, false), None);
    }
}
