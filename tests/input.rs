use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tili::actions::{Action, ResizeModeAction, action_for_key, resize_action_for_key};
use tili::input::{encode_key, encode_paste};
use tili::tiling::{Direction, Orientation, Resize};

fn alt(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::ALT)
}

#[test]
fn core_alt_bindings_follow_the_custom_i3_layout() {
    assert_eq!(action_for_key(alt(KeyCode::Enter)), Some(Action::OpenPane));
    assert_eq!(
        action_for_key(alt(KeyCode::Char('e'))),
        Some(Action::ToggleSplit)
    );
    assert_eq!(
        action_for_key(alt(KeyCode::Char('h'))),
        Some(Action::SetSplit(Orientation::Horizontal))
    );
    assert_eq!(
        action_for_key(alt(KeyCode::Char('v'))),
        Some(Action::SetSplit(Orientation::Vertical))
    );
    assert_eq!(
        action_for_key(alt(KeyCode::Char('j'))),
        Some(Action::Focus(Direction::Left))
    );
    assert_eq!(
        action_for_key(alt(KeyCode::Char('k'))),
        Some(Action::Focus(Direction::Down))
    );
    assert_eq!(
        action_for_key(alt(KeyCode::Char('l'))),
        Some(Action::Focus(Direction::Up))
    );
    assert_eq!(
        action_for_key(alt(KeyCode::Char(';'))),
        Some(Action::Focus(Direction::Right))
    );
    assert_eq!(
        action_for_key(alt(KeyCode::Char('d'))),
        Some(Action::OpenLauncher)
    );
}

#[test]
fn shifted_directions_move_panes_instead_of_changing_focus() {
    let key = KeyEvent::new(KeyCode::Char('J'), KeyModifiers::ALT | KeyModifiers::SHIFT);
    assert_eq!(action_for_key(key), Some(Action::Move(Direction::Left)));
    assert_eq!(
        action_for_key(KeyEvent::new(
            KeyCode::Char(':'),
            KeyModifiers::ALT | KeyModifiers::SHIFT,
        )),
        Some(Action::Move(Direction::Right))
    );
}

#[test]
fn number_bindings_switch_and_move_to_all_ten_workspaces() {
    assert_eq!(
        action_for_key(alt(KeyCode::Char('1'))),
        Some(Action::SwitchWorkspace(0))
    );
    assert_eq!(
        action_for_key(alt(KeyCode::Char('0'))),
        Some(Action::SwitchWorkspace(9))
    );
    assert_eq!(
        action_for_key(KeyEvent::new(
            KeyCode::Char('3'),
            KeyModifiers::ALT | KeyModifiers::SHIFT,
        )),
        Some(Action::MoveToWorkspace(2))
    );
    assert_eq!(
        action_for_key(KeyEvent::new(
            KeyCode::Char(')'),
            KeyModifiers::ALT | KeyModifiers::SHIFT,
        )),
        Some(Action::MoveToWorkspace(9))
    );
    assert_eq!(
        action_for_key(alt(KeyCode::Char('!'))),
        Some(Action::MoveToWorkspace(0))
    );
    assert_eq!(
        action_for_key(alt(KeyCode::Char('$'))),
        Some(Action::MoveToWorkspace(3))
    );
    assert_eq!(
        action_for_key(alt(KeyCode::Char(')'))),
        Some(Action::MoveToWorkspace(9))
    );
    assert_eq!(
        action_for_key(KeyEvent::new(
            KeyCode::Char('C'),
            KeyModifiers::ALT | KeyModifiers::SHIFT,
        )),
        Some(Action::ReloadConfig)
    );
}

#[test]
fn resize_mode_uses_the_requested_custom_directions() {
    assert_eq!(
        resize_action_for_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE)),
        Some(ResizeModeAction::Resize(Resize::ShrinkWidth))
    );
    assert_eq!(
        resize_action_for_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
        Some(ResizeModeAction::Resize(Resize::GrowHeight))
    );
    assert_eq!(
        resize_action_for_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE)),
        Some(ResizeModeAction::Resize(Resize::ShrinkHeight))
    );
    assert_eq!(
        resize_action_for_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)),
        Some(ResizeModeAction::Resize(Resize::GrowWidth))
    );
    assert_eq!(
        resize_action_for_key(alt(KeyCode::Char('r'))),
        Some(ResizeModeAction::Exit)
    );
}

#[test]
fn terminal_input_encoding_honors_application_cursor_and_bracketed_paste_modes() {
    let mut parser = tastty_core::Parser::new(tastty_core::TerminalSize { rows: 1, cols: 1 }, 0);
    assert_eq!(
        encode_key(alt(KeyCode::Char('x')), parser.screen()),
        Some(b"\x1bx".to_vec())
    );
    assert_eq!(
        encode_key(
            KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
            parser.screen()
        ),
        Some(b"\x1b[A".to_vec())
    );
    parser.process(b"\x1b[?1h");
    assert_eq!(
        encode_key(
            KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
            parser.screen()
        ),
        Some(b"\x1bOA".to_vec())
    );
    assert_eq!(encode_paste("hello", false), b"hello".to_vec());
    assert_eq!(
        encode_paste("hello", true),
        b"\x1b[200~hello\x1b[201~".to_vec()
    );
}

#[test]
fn modified_navigation_keys_preserve_ctrl_shift_and_alt() {
    let mut parser = tastty_core::Parser::new(tastty_core::TerminalSize { rows: 1, cols: 1 }, 0);
    for application_cursor in [false, true] {
        if application_cursor {
            parser.process(b"\x1b[?1h");
        }
        for (code, modifiers, expected) in [
            (KeyCode::Left, KeyModifiers::CONTROL, "\x1b[1;5D"),
            (KeyCode::Right, KeyModifiers::CONTROL, "\x1b[1;5C"),
            (KeyCode::Up, KeyModifiers::CONTROL, "\x1b[1;5A"),
            (KeyCode::Down, KeyModifiers::CONTROL, "\x1b[1;5B"),
            (KeyCode::Left, KeyModifiers::SHIFT, "\x1b[1;2D"),
            (
                KeyCode::Right,
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
                "\x1b[1;6C",
            ),
            (KeyCode::Left, KeyModifiers::ALT, "\x1b[1;3D"),
            (
                KeyCode::Right,
                KeyModifiers::CONTROL | KeyModifiers::ALT,
                "\x1b[1;7C",
            ),
            (KeyCode::Home, KeyModifiers::CONTROL, "\x1b[1;5H"),
            (KeyCode::End, KeyModifiers::CONTROL, "\x1b[1;5F"),
            (KeyCode::Insert, KeyModifiers::SHIFT, "\x1b[2;2~"),
            (KeyCode::Delete, KeyModifiers::CONTROL, "\x1b[3;5~"),
            (KeyCode::PageUp, KeyModifiers::CONTROL, "\x1b[5;5~"),
            (KeyCode::PageDown, KeyModifiers::CONTROL, "\x1b[6;5~"),
        ] {
            let key = KeyEvent::new(code, modifiers);
            assert_eq!(
                encode_key(key, parser.screen()),
                Some(expected.as_bytes().to_vec()),
                "{key:?}, application_cursor={application_cursor}"
            );
        }
    }
    let mut key = KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL);
    key.kind = crossterm::event::KeyEventKind::Repeat;
    assert_eq!(
        encode_key(key, parser.screen()),
        Some(b"\x1b[1;5D".to_vec())
    );
    key.kind = crossterm::event::KeyEventKind::Release;
    assert_eq!(encode_key(key, parser.screen()), None);
}
