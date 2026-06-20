//! Maps a (mode, key) pair to a high-level [`Action`]. Pure and stateless;
//! the two-key chord `gg` emits [`Action::Prefix`] that `App` completes.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Mode {
    #[default]
    Normal,
    Insert,
    Address,
    Help,
    Hint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Action {
    ScrollLineDown,
    ScrollLineUp,
    ScrollHalfDown,
    ScrollHalfUp,
    ScrollPageDown,
    ScrollPageUp,
    GoBottom,
    Prefix(char),
    HistoryBack,
    HistoryForward,
    OpenAddress,
    Reload,
    EnterInsert,
    EnterHint,
    OpenHelp,
    AddressChar(char),
    AddressBackspace,
    AddressConfirm,
    HintKey(char),
    HintBackspace,
    Escape,
    Quit,
    Ignore,
}

/// Maps a key event in `mode` to an [`Action`].
pub(crate) fn map(mode: Mode, key: KeyEvent) -> Action {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match mode {
        Mode::Normal => map_normal(ctrl, key.code),
        Mode::Hint => map_hint(ctrl, key.code),
        Mode::Insert => match key.code {
            KeyCode::Esc => Action::Escape,
            _ => Action::Ignore,
        },
        Mode::Address => match key.code {
            KeyCode::Char('c') if ctrl => Action::Quit,
            KeyCode::Esc => Action::Escape,
            KeyCode::Enter => Action::AddressConfirm,
            KeyCode::Backspace => Action::AddressBackspace,
            KeyCode::Char(c) => Action::AddressChar(c),
            _ => Action::Ignore,
        },
        Mode::Help => match key.code {
            KeyCode::Char('c') if ctrl => Action::Quit,
            KeyCode::Esc => Action::Escape,
            KeyCode::Char('q') => Action::Escape,
            _ => Action::Ignore,
        },
    }
}

fn map_normal(ctrl: bool, code: KeyCode) -> Action {
    match code {
        KeyCode::Char('c') if ctrl => Action::Quit,
        KeyCode::Char('d') if ctrl => Action::ScrollHalfDown,
        KeyCode::Char('u') if ctrl => Action::ScrollHalfUp,
        KeyCode::Char('f') if ctrl => Action::ScrollPageDown,
        KeyCode::Char('b') if ctrl => Action::ScrollPageUp,
        _ if ctrl => Action::Ignore,
        KeyCode::Char('j') | KeyCode::Down => Action::ScrollLineDown,
        KeyCode::Char('k') | KeyCode::Up => Action::ScrollLineUp,
        KeyCode::Char(' ') => Action::ScrollHalfDown,
        KeyCode::PageDown => Action::ScrollPageDown,
        KeyCode::PageUp => Action::ScrollPageUp,
        KeyCode::Char('G') => Action::GoBottom,
        KeyCode::Char('g') => Action::Prefix('g'),
        KeyCode::Char('H') => Action::HistoryBack,
        KeyCode::Char('L') => Action::HistoryForward,
        KeyCode::Char('o') | KeyCode::Char(':') => Action::OpenAddress,
        KeyCode::Char('r') => Action::Reload,
        KeyCode::Char('i') => Action::EnterInsert,
        KeyCode::Char('f') => Action::EnterHint,
        KeyCode::Char('?') => Action::OpenHelp,
        KeyCode::Char('q') => Action::Quit,
        _ => Action::Ignore,
    }
}

fn map_hint(ctrl: bool, code: KeyCode) -> Action {
    match code {
        KeyCode::Char('c') if ctrl => Action::Quit,
        _ if ctrl => Action::Ignore,
        KeyCode::Esc => Action::Escape,
        KeyCode::Backspace => Action::HintBackspace,
        KeyCode::Char(c) => Action::HintKey(c),
        _ => Action::Ignore,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }
    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }
    fn special(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn normal_scroll_keys() {
        assert_eq!(map(Mode::Normal, key('j')), Action::ScrollLineDown);
        assert_eq!(
            map(Mode::Normal, special(KeyCode::Down)),
            Action::ScrollLineDown
        );
        assert_eq!(map(Mode::Normal, key('k')), Action::ScrollLineUp);
        assert_eq!(
            map(Mode::Normal, special(KeyCode::Up)),
            Action::ScrollLineUp
        );
        assert_eq!(map(Mode::Normal, ctrl('d')), Action::ScrollHalfDown);
        assert_eq!(map(Mode::Normal, key(' ')), Action::ScrollHalfDown);
        assert_eq!(map(Mode::Normal, ctrl('u')), Action::ScrollHalfUp);
        assert_eq!(map(Mode::Normal, ctrl('f')), Action::ScrollPageDown);
        assert_eq!(
            map(Mode::Normal, special(KeyCode::PageDown)),
            Action::ScrollPageDown
        );
        assert_eq!(map(Mode::Normal, ctrl('b')), Action::ScrollPageUp);
        assert_eq!(
            map(Mode::Normal, special(KeyCode::PageUp)),
            Action::ScrollPageUp
        );
    }

    #[test]
    fn normal_navigation_and_modes() {
        assert_eq!(map(Mode::Normal, key('G')), Action::GoBottom);
        assert_eq!(map(Mode::Normal, key('g')), Action::Prefix('g'));
        assert_eq!(map(Mode::Normal, key('H')), Action::HistoryBack);
        assert_eq!(map(Mode::Normal, key('L')), Action::HistoryForward);
        assert_eq!(map(Mode::Normal, key('o')), Action::OpenAddress);
        assert_eq!(map(Mode::Normal, key(':')), Action::OpenAddress);
        assert_eq!(map(Mode::Normal, key('r')), Action::Reload);
        assert_eq!(map(Mode::Normal, key('i')), Action::EnterInsert);
        assert_eq!(map(Mode::Normal, key('?')), Action::OpenHelp);
        assert_eq!(map(Mode::Normal, key('q')), Action::Quit);
        assert_eq!(map(Mode::Normal, ctrl('c')), Action::Quit);
    }

    #[test]
    fn normal_unrecognized_key_is_ignore() {
        assert_eq!(map(Mode::Normal, key('z')), Action::Ignore);
        assert_eq!(map(Mode::Normal, ctrl('x')), Action::Ignore);
    }

    #[test]
    fn insert_mode_only_intercepts_esc() {
        assert_eq!(map(Mode::Insert, special(KeyCode::Esc)), Action::Escape);
        assert_eq!(map(Mode::Insert, key('a')), Action::Ignore);
        assert_eq!(map(Mode::Insert, key('j')), Action::Ignore);
    }

    #[test]
    fn address_mode_keys() {
        assert_eq!(map(Mode::Address, key('h')), Action::AddressChar('h'));
        assert_eq!(map(Mode::Address, key('/')), Action::AddressChar('/'));
        assert_eq!(
            map(Mode::Address, special(KeyCode::Backspace)),
            Action::AddressBackspace
        );
        assert_eq!(
            map(Mode::Address, special(KeyCode::Enter)),
            Action::AddressConfirm
        );
        assert_eq!(map(Mode::Address, special(KeyCode::Esc)), Action::Escape);
    }

    #[test]
    fn help_mode_esc_and_q_close() {
        assert_eq!(map(Mode::Help, special(KeyCode::Esc)), Action::Escape);
        assert_eq!(map(Mode::Help, key('q')), Action::Escape);
        assert_eq!(map(Mode::Help, key('j')), Action::Ignore);
    }

    #[test]
    fn normal_f_enters_hint_mode() {
        assert_eq!(map(Mode::Normal, key('f')), Action::EnterHint);
    }

    #[test]
    fn hint_mode_printable_char_is_hint_key() {
        assert_eq!(map(Mode::Hint, key('a')), Action::HintKey('a'));
        assert_eq!(map(Mode::Hint, key('s')), Action::HintKey('s'));
    }

    #[test]
    fn hint_mode_backspace_and_escape() {
        assert_eq!(
            map(Mode::Hint, special(KeyCode::Backspace)),
            Action::HintBackspace
        );
        assert_eq!(map(Mode::Hint, special(KeyCode::Esc)), Action::Escape);
    }

    #[test]
    fn hint_mode_ctrl_c_quits_and_other_ctrl_ignored() {
        assert_eq!(map(Mode::Hint, ctrl('c')), Action::Quit);
        assert_eq!(map(Mode::Hint, ctrl('d')), Action::Ignore);
    }
}
