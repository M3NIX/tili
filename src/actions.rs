use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::tiling::{Direction, Orientation, Resize};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    OpenPane,
    OpenPaneInWorkingDirectory,
    ClosePane,
    SetSplit(Orientation),
    ToggleSplit,
    Focus(Direction),
    Move(Direction),
    EnterResizeMode,
    ToggleFullscreen,
    OpenLauncher,
    SwitchWorkspace(usize),
    MoveToWorkspace(usize),
    ReloadConfig,
    RequestExit,
}

pub fn action_for_key(key: KeyEvent) -> Option<Action> {
    if matches!(key.kind, KeyEventKind::Release) || !key.modifiers.contains(KeyModifiers::ALT) {
        return None;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return None;
    }
    let shifted = key.modifiers.contains(KeyModifiers::SHIFT);

    if let Some((workspace, implied_shift)) = workspace_for_key(key.code) {
        return Some(if shifted || implied_shift {
            Action::MoveToWorkspace(workspace)
        } else {
            Action::SwitchWorkspace(workspace)
        });
    }

    match (key.code, shifted) {
        (KeyCode::Char('j' | 'J') | KeyCode::Left, true) => Some(Action::Move(Direction::Left)),
        (KeyCode::Char('k' | 'K') | KeyCode::Down, true) => Some(Action::Move(Direction::Down)),
        (KeyCode::Char('l' | 'L') | KeyCode::Up, true) => Some(Action::Move(Direction::Up)),
        (KeyCode::Char(';' | ':') | KeyCode::Right, true) => Some(Action::Move(Direction::Right)),
        (KeyCode::Char('c' | 'C'), true) => Some(Action::ReloadConfig),
        (KeyCode::Char('e' | 'E'), true) => Some(Action::RequestExit),
        (_, true) => None,
        (KeyCode::Enter, false) => Some(Action::OpenPane),
        (KeyCode::Backspace, false) => Some(Action::ClosePane),
        (KeyCode::Char('t' | 'T'), false) => Some(Action::OpenPaneInWorkingDirectory),
        (KeyCode::Char('h' | 'H'), false) => Some(Action::SetSplit(Orientation::Horizontal)),
        (KeyCode::Char('v' | 'V'), false) => Some(Action::SetSplit(Orientation::Vertical)),
        (KeyCode::Char('e' | 'E'), false) => Some(Action::ToggleSplit),
        (KeyCode::Char('f' | 'F'), false) => Some(Action::ToggleFullscreen),
        (KeyCode::Char('d' | 'D'), false) => Some(Action::OpenLauncher),
        (KeyCode::Char('r' | 'R'), false) => Some(Action::EnterResizeMode),
        (KeyCode::Char('j' | 'J') | KeyCode::Left, false) => Some(Action::Focus(Direction::Left)),
        (KeyCode::Char('k' | 'K') | KeyCode::Down, false) => Some(Action::Focus(Direction::Down)),
        (KeyCode::Char('l' | 'L') | KeyCode::Up, false) => Some(Action::Focus(Direction::Up)),
        (KeyCode::Char(';') | KeyCode::Right, false) => Some(Action::Focus(Direction::Right)),
        _ => None,
    }
}

fn workspace_for_key(code: KeyCode) -> Option<(usize, bool)> {
    let (number, implied_shift) = match code {
        KeyCode::Char('1') => (1, false),
        KeyCode::Char('!') => (1, true),
        KeyCode::Char('2') => (2, false),
        KeyCode::Char('@') => (2, true),
        KeyCode::Char('3') => (3, false),
        KeyCode::Char('#') => (3, true),
        KeyCode::Char('4') => (4, false),
        KeyCode::Char('$') => (4, true),
        KeyCode::Char('5') => (5, false),
        KeyCode::Char('%') => (5, true),
        KeyCode::Char('6') => (6, false),
        KeyCode::Char('^') => (6, true),
        KeyCode::Char('7') => (7, false),
        KeyCode::Char('&') => (7, true),
        KeyCode::Char('8') => (8, false),
        KeyCode::Char('*') => (8, true),
        KeyCode::Char('9') => (9, false),
        KeyCode::Char('(') => (9, true),
        KeyCode::Char('0') => (10, false),
        KeyCode::Char(')') => (10, true),
        _ => return None,
    };
    Some((number - 1, implied_shift))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResizeModeAction {
    Resize(Resize),
    Exit,
}

pub fn resize_action_for_key(key: KeyEvent) -> Option<ResizeModeAction> {
    if matches!(key.kind, KeyEventKind::Release) {
        return None;
    }
    if key.modifiers.contains(KeyModifiers::ALT) && matches!(key.code, KeyCode::Char('r' | 'R')) {
        return Some(ResizeModeAction::Exit);
    }
    if key
        .modifiers
        .intersects(KeyModifiers::ALT | KeyModifiers::CONTROL)
    {
        return None;
    }

    match key.code {
        KeyCode::Enter | KeyCode::Esc => Some(ResizeModeAction::Exit),
        KeyCode::Char('j' | 'J') | KeyCode::Left => {
            Some(ResizeModeAction::Resize(Resize::ShrinkWidth))
        }
        KeyCode::Char('k' | 'K') | KeyCode::Down => {
            Some(ResizeModeAction::Resize(Resize::GrowHeight))
        }
        KeyCode::Char('l' | 'L') | KeyCode::Up => {
            Some(ResizeModeAction::Resize(Resize::ShrinkHeight))
        }
        KeyCode::Char(';' | ':') | KeyCode::Right => {
            Some(ResizeModeAction::Resize(Resize::GrowWidth))
        }
        _ => None,
    }
}
