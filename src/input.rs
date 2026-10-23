use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

pub fn encode_key(key: KeyEvent, application_cursor: bool) -> Option<Vec<u8>> {
    if matches!(key.kind, KeyEventKind::Release) {
        return None;
    }

    let mut bytes = Vec::with_capacity(8);
    if key.modifiers.contains(KeyModifiers::ALT) {
        bytes.push(0x1b);
    }

    match key.code {
        KeyCode::Char(character) if key.modifiers.contains(KeyModifiers::CONTROL) => {
            bytes.push(control_byte(character)?);
        }
        KeyCode::Char(character) => {
            let mut encoded = [0; 4];
            bytes.extend_from_slice(character.encode_utf8(&mut encoded).as_bytes());
        }
        KeyCode::Enter => bytes.push(b'\r'),
        KeyCode::Backspace => bytes.push(0x7f),
        KeyCode::Tab => bytes.push(b'\t'),
        KeyCode::BackTab => bytes.extend_from_slice(b"\x1b[Z"),
        KeyCode::Esc => bytes.push(0x1b),
        KeyCode::Left => bytes.extend_from_slice(cursor_sequence(b'D', application_cursor)),
        KeyCode::Right => bytes.extend_from_slice(cursor_sequence(b'C', application_cursor)),
        KeyCode::Up => bytes.extend_from_slice(cursor_sequence(b'A', application_cursor)),
        KeyCode::Down => bytes.extend_from_slice(cursor_sequence(b'B', application_cursor)),
        KeyCode::Home => bytes.extend_from_slice(if application_cursor {
            b"\x1bOH"
        } else {
            b"\x1b[H"
        }),
        KeyCode::End => bytes.extend_from_slice(if application_cursor {
            b"\x1bOF"
        } else {
            b"\x1b[F"
        }),
        KeyCode::Insert => bytes.extend_from_slice(b"\x1b[2~"),
        KeyCode::Delete => bytes.extend_from_slice(b"\x1b[3~"),
        KeyCode::PageUp => bytes.extend_from_slice(b"\x1b[5~"),
        KeyCode::PageDown => bytes.extend_from_slice(b"\x1b[6~"),
        KeyCode::F(number) => bytes.extend_from_slice(function_key_sequence(number)?),
        KeyCode::Null => bytes.push(0),
        KeyCode::CapsLock
        | KeyCode::ScrollLock
        | KeyCode::NumLock
        | KeyCode::PrintScreen
        | KeyCode::Pause
        | KeyCode::Menu
        | KeyCode::KeypadBegin
        | KeyCode::Media(_)
        | KeyCode::Modifier(_) => return None,
    }

    Some(bytes)
}

pub fn encode_paste(text: &str, bracketed: bool) -> Vec<u8> {
    if bracketed {
        let mut bytes = Vec::with_capacity(text.len() + 12);
        bytes.extend_from_slice(b"\x1b[200~");
        bytes.extend_from_slice(text.as_bytes());
        bytes.extend_from_slice(b"\x1b[201~");
        bytes
    } else {
        text.as_bytes().to_vec()
    }
}

fn control_byte(character: char) -> Option<u8> {
    match character {
        '@' | ' ' | '2' => Some(0),
        'a'..='z' => Some(character as u8 - b'a' + 1),
        'A'..='Z' => Some(character as u8 - b'A' + 1),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' => Some(0x1e),
        '_' | '7' => Some(0x1f),
        '?' | '8' => Some(0x7f),
        _ => None,
    }
}

fn cursor_sequence(final_byte: u8, application_cursor: bool) -> &'static [u8] {
    match (final_byte, application_cursor) {
        (b'A', false) => b"\x1b[A",
        (b'B', false) => b"\x1b[B",
        (b'C', false) => b"\x1b[C",
        (b'D', false) => b"\x1b[D",
        (b'A', true) => b"\x1bOA",
        (b'B', true) => b"\x1bOB",
        (b'C', true) => b"\x1bOC",
        (b'D', true) => b"\x1bOD",
        _ => unreachable!("cursor sequence called with an unsupported final byte"),
    }
}

fn function_key_sequence(number: u8) -> Option<&'static [u8]> {
    match number {
        1 => Some(b"\x1bOP"),
        2 => Some(b"\x1bOQ"),
        3 => Some(b"\x1bOR"),
        4 => Some(b"\x1bOS"),
        5 => Some(b"\x1b[15~"),
        6 => Some(b"\x1b[17~"),
        7 => Some(b"\x1b[18~"),
        8 => Some(b"\x1b[19~"),
        9 => Some(b"\x1b[20~"),
        10 => Some(b"\x1b[21~"),
        11 => Some(b"\x1b[23~"),
        12 => Some(b"\x1b[24~"),
        _ => None,
    }
}
