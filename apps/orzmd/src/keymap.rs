//! Maps a (mode, key) pair to a high-level [`Action`]. Pure and stateless;
//! two-key chords (`gg`, `]]`, `[[`) emit a [`Action::Prefix`] that `App`
//! completes.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui_orzma::KeyChord;

/// The current input mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Mode {
    /// Scrolling / navigation.
    #[default]
    Normal,
    /// Outline panel is open and focused.
    Outline,
    /// Search query is being typed.
    Search,
}

/// A high-level action produced by a key in some mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Action {
    Quit,
    Reload,
    ScrollLineDown,
    ScrollLineUp,
    ScrollHalfDown,
    ScrollHalfUp,
    ScrollPageDown,
    ScrollPageUp,
    GoBottom,
    /// Pop the navigation back stack.
    Back,
    /// First key of a two-key chord (`g`, `]`, `[`).
    Prefix(char),
    ToggleOutline,
    OutlineMoveDown,
    OutlineMoveUp,
    OutlineConfirm,
    EnterSearch,
    SearchChar(char),
    SearchBackspace,
    SearchConfirm,
    SearchNext,
    SearchPrev,
    Escape,
    Ignore,
}

/// Maps a key event in `mode` to an [`Action`].
pub(crate) fn map(mode: Mode, key: KeyEvent) -> Action {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match mode {
        Mode::Search => match key.code {
            KeyCode::Esc => Action::Escape,
            KeyCode::Enter => Action::SearchConfirm,
            KeyCode::Backspace => Action::SearchBackspace,
            KeyCode::Char(c) => Action::SearchChar(c),
            _ => Action::Ignore,
        },
        Mode::Outline => match key.code {
            KeyCode::Esc => Action::Escape,
            KeyCode::Tab => Action::ToggleOutline,
            KeyCode::Enter => Action::OutlineConfirm,
            KeyCode::Char('o') => Action::ToggleOutline,
            KeyCode::Char('j') | KeyCode::Down => Action::OutlineMoveDown,
            KeyCode::Char('k') | KeyCode::Up => Action::OutlineMoveUp,
            KeyCode::Char('q') if !ctrl => Action::Quit,
            KeyCode::Char('c') if ctrl => Action::Quit,
            _ => Action::Ignore,
        },
        Mode::Normal => map_normal(ctrl, key.code),
    }
}

/// The chords orzmd's Normal and Outline keymaps use, passed through to the
/// TUI while the page holds keyboard focus.
///
/// Ctrl+C is left out so that, while the page is focused, it copies the
/// page's selection instead of quitting.
pub(crate) fn forward_chords() -> Vec<KeyChord> {
    let plain = |code| KeyChord {
        mods: KeyModifiers::NONE,
        code,
    };
    let shift = |c| KeyChord {
        mods: KeyModifiers::SHIFT,
        code: KeyCode::Char(c),
    };
    let ctrl = |c| KeyChord {
        mods: KeyModifiers::CONTROL,
        code: KeyCode::Char(c),
    };
    vec![
        plain(KeyCode::Esc),
        plain(KeyCode::Enter),
        plain(KeyCode::Tab),
        plain(KeyCode::Backspace),
        plain(KeyCode::Up),
        plain(KeyCode::Down),
        plain(KeyCode::PageUp),
        plain(KeyCode::PageDown),
        plain(KeyCode::Char(' ')),
        plain(KeyCode::Char('j')),
        plain(KeyCode::Char('k')),
        plain(KeyCode::Char('q')),
        plain(KeyCode::Char('r')),
        plain(KeyCode::Char('g')),
        plain(KeyCode::Char('o')),
        plain(KeyCode::Char('n')),
        plain(KeyCode::Char('/')),
        plain(KeyCode::Char('[')),
        plain(KeyCode::Char(']')),
        shift('g'),
        shift('n'),
        ctrl('d'),
        ctrl('u'),
        ctrl('f'),
        ctrl('b'),
        ctrl('o'),
    ]
}

fn map_normal(ctrl: bool, code: KeyCode) -> Action {
    match code {
        KeyCode::Char('c') if ctrl => Action::Quit,
        KeyCode::Char('d') if ctrl => Action::ScrollHalfDown,
        KeyCode::Char('u') if ctrl => Action::ScrollHalfUp,
        KeyCode::Char('f') if ctrl => Action::ScrollPageDown,
        KeyCode::Char('b') if ctrl => Action::ScrollPageUp,
        KeyCode::Char('o') if ctrl => Action::Back,
        _ if ctrl => Action::Ignore,
        KeyCode::Char('q') => Action::Quit,
        KeyCode::Char('r') => Action::Reload,
        KeyCode::Backspace => Action::Back,
        KeyCode::Char('j') | KeyCode::Down => Action::ScrollLineDown,
        KeyCode::Char('k') | KeyCode::Up => Action::ScrollLineUp,
        KeyCode::Char(' ') | KeyCode::PageDown => Action::ScrollPageDown,
        KeyCode::PageUp => Action::ScrollPageUp,
        KeyCode::Char('G') => Action::GoBottom,
        KeyCode::Char('g') => Action::Prefix('g'),
        KeyCode::Char(']') => Action::Prefix(']'),
        KeyCode::Char('[') => Action::Prefix('['),
        KeyCode::Char('o') | KeyCode::Tab => Action::ToggleOutline,
        KeyCode::Char('/') => Action::EnterSearch,
        KeyCode::Char('n') => Action::SearchNext,
        KeyCode::Char('N') => Action::SearchPrev,
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

    #[test]
    fn normal_scrolling() {
        assert_eq!(map(Mode::Normal, key('j')), Action::ScrollLineDown);
        assert_eq!(map(Mode::Normal, key('k')), Action::ScrollLineUp);
        assert_eq!(map(Mode::Normal, ctrl('d')), Action::ScrollHalfDown);
        assert_eq!(map(Mode::Normal, ctrl('u')), Action::ScrollHalfUp);
    }

    #[test]
    fn normal_prefixes_and_bottom() {
        assert_eq!(map(Mode::Normal, key('g')), Action::Prefix('g'));
        assert_eq!(map(Mode::Normal, key('G')), Action::GoBottom);
        assert_eq!(map(Mode::Normal, key(']')), Action::Prefix(']'));
        assert_eq!(map(Mode::Normal, key('[')), Action::Prefix('['));
    }

    #[test]
    fn normal_search_nav_and_modes() {
        assert_eq!(map(Mode::Normal, key('/')), Action::EnterSearch);
        assert_eq!(map(Mode::Normal, key('n')), Action::SearchNext);
        assert_eq!(map(Mode::Normal, key('N')), Action::SearchPrev);
        assert_eq!(map(Mode::Normal, key('o')), Action::ToggleOutline);
        assert_eq!(map(Mode::Normal, key('q')), Action::Quit);
        assert_eq!(map(Mode::Normal, ctrl('c')), Action::Quit);
        assert_eq!(map(Mode::Normal, key('r')), Action::Reload);
    }

    #[test]
    fn search_mode_typing() {
        assert_eq!(map(Mode::Search, key('a')), Action::SearchChar('a'));
        assert_eq!(
            map(
                Mode::Search,
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
            ),
            Action::SearchConfirm
        );
        assert_eq!(
            map(
                Mode::Search,
                KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)
            ),
            Action::SearchBackspace
        );
        assert_eq!(
            map(
                Mode::Search,
                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
            ),
            Action::Escape
        );
    }

    #[test]
    fn outline_mode_navigation() {
        assert_eq!(map(Mode::Outline, key('j')), Action::OutlineMoveDown);
        assert_eq!(map(Mode::Outline, key('k')), Action::OutlineMoveUp);
        assert_eq!(
            map(
                Mode::Outline,
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
            ),
            Action::OutlineConfirm
        );
        assert_eq!(map(Mode::Outline, key('o')), Action::ToggleOutline);
        assert_eq!(
            map(
                Mode::Outline,
                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
            ),
            Action::Escape
        );
    }

    #[test]
    fn normal_backspace_and_ctrl_o_go_back() {
        assert_eq!(
            map(
                Mode::Normal,
                KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)
            ),
            Action::Back
        );
        assert_eq!(map(Mode::Normal, ctrl('o')), Action::Back);
    }

    #[test]
    fn search_backspace_still_edits_query() {
        assert_eq!(
            map(
                Mode::Search,
                KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)
            ),
            Action::SearchBackspace
        );
    }

    /// Asserts that every forwarded chord drives an action in Normal or
    /// Outline mode, and that Ctrl+C is not forwarded.
    ///
    /// Case: the user clicks the page and keeps navigating with the keys the
    /// TUI documents, or selects text and presses Ctrl+C to copy it.
    #[test]
    fn every_forwarded_chord_drives_a_normal_or_outline_action() {
        for chord in forward_chords() {
            let code = match chord.code {
                KeyCode::Char(c) if chord.mods.contains(KeyModifiers::SHIFT) => {
                    KeyCode::Char(c.to_ascii_uppercase())
                }
                code => code,
            };
            let event = KeyEvent::new(code, chord.mods);
            assert!(
                map(Mode::Normal, event) != Action::Ignore
                    || map(Mode::Outline, event) != Action::Ignore,
                "{chord:?} drives nothing"
            );
        }
        assert!(!forward_chords().contains(&KeyChord {
            mods: KeyModifiers::CONTROL,
            code: KeyCode::Char('c'),
        }));
    }
}
