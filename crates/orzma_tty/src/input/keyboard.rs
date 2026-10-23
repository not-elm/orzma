//! Pure VT-encoder for key input: translates a logical key and its
//! modifiers into the byte sequence the PTY expects.

mod cursor;
mod editing;
mod function;
mod keypad;
mod kitty;
mod text;

pub use cursor::CursorKey;
pub use editing::EditingKey;
pub use function::FunctionKey;
pub use keypad::KeypadKey;
use orzma_vt::prelude::{KittyKeyboardFlags, Vt};
pub use text::KeyText;

/// Subset of keys the terminal input codec understands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalKey {
    /// UTF-8 text (single char or multi-codepoint composition).
    Character(KeyText),
    /// A numeric keypad key.
    Keypad {
        /// The physical key.
        key: KeypadKey,
        /// What the key typed, if anything.
        text: Option<KeyText>,
    },
    Function(FunctionKey),
    Cursor(CursorKey),
    Editing(EditingKey),
    Enter,
    Backspace,
    Tab,
    Escape,
    /// A key press that typed no character under the modifiers held and has no name.
    ///
    /// It sends no bytes in the legacy form.
    Unidentified {
        /// The character the key types when no modifier is held.
        unshifted: char,
    },
}

/// Modifier flags carried alongside `TerminalKey`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TerminalModifiers {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
}

impl TerminalModifiers {
    /// Returns the xterm modifier code a modified key carries as a
    /// parameter, 1 plus Shift 1, Alt 2, and Ctrl 4, or `None` when none of
    /// the three is held. Meta does not count toward the code.
    fn xterm_code(&self) -> Option<u8> {
        let code = 1 + u8::from(self.shift) + 2 * u8::from(self.alt) + 4 * u8::from(self.ctrl);
        (code > 1).then_some(code)
    }

    /// Returns the kitty keyboard protocol's modifier parameter, 1 plus
    /// Shift 1, Alt 2, Ctrl 4, and Meta 8 (the protocol's Super bit), or
    /// `None` when no modifier is held.
    fn kitty_code(&self) -> Option<u8> {
        let code = self.xterm_code().unwrap_or(1) + 8 * u8::from(self.meta);
        (code > 1).then_some(code)
    }
}

/// The terminal state a key's encoding depends on: DECCKM and the kitty
/// keyboard flags of the screen on show.
///
/// # References
///
/// - <https://sw.kovidgoyal.net/kitty/keyboard-protocol/>, "Progressive
///   enhancement".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEncodingModes {
    app_cursor: bool,
    kitty: KittyKeyboardFlags,
}

impl KeyEncodingModes {
    /// Reads the modes a key press is encoded under from `vt`.
    pub fn read(vt: &impl Vt) -> Self {
        Self {
            app_cursor: vt.modes().app_cursor,
            kitty: vt.kitty_keyboard_flags(),
        }
    }
}

/// Translates a logical key + modifiers into the bytes the PTY expects.
///
/// While `modes` holds the kitty keyboard protocol's flag 1 (disambiguate
/// escape codes), the key takes that protocol's form, which DECCKM does not
/// affect. Otherwise it takes the legacy form described below, and a key
/// that has none encodes to no bytes.
///
/// The cursor keys — the arrows plus Home and End — honour
/// DECCKM in `modes` when neither Shift, Alt, nor Ctrl is held:
/// `ESC [ A/B/C/D/H/F` in normal mode, `ESC O A/B/C/D/H/F` in application
/// mode. With any of the three held, they send `CSI 1 ; m A/B/C/D/H/F` in
/// either mode. The editing keypad (Insert, Delete, PageUp, PageDown) is
/// unaffected by DECCKM: it sends `CSI n ~`, or `CSI n ; m ~` with a
/// modifier held.
///
/// Tab sends HT, and CBT (`CSI Z`, the terminfo `kcbt` string) when Shift
/// is the only modifier held; other modifiers leave Tab as HT.
pub(super) fn encode_key(
    key: &TerminalKey,
    mods: &TerminalModifiers,
    modes: KeyEncodingModes,
) -> Vec<u8> {
    if modes.kitty.contains(KittyKeyboardFlags::DISAMBIGUATE) {
        return kitty::encode(key, mods);
    }
    match key {
        TerminalKey::Character(text) => text.encode(mods),
        TerminalKey::Keypad { key, text } => {
            key.encode_legacy(text.as_ref(), mods, modes.app_cursor)
        }
        TerminalKey::Function(function) => function.encode(mods),
        TerminalKey::Cursor(cursor) => cursor.encode(mods, modes.app_cursor),
        TerminalKey::Editing(editing) => editing.encode(mods),
        TerminalKey::Enter => vec![0x0d],
        TerminalKey::Backspace => vec![0x7f],
        TerminalKey::Tab if mods.shift && !mods.ctrl && !mods.alt && !mods.meta => {
            b"\x1b[Z".to_vec()
        }
        TerminalKey::Tab => vec![0x09],
        TerminalKey::Escape => vec![0x1b],
        TerminalKey::Unidentified { .. } => Vec::new(),
    }
}

/// Builds `CSI number ; code final_byte`, the xterm form a key sends with
/// Shift, Alt, or Ctrl held, where `code` is the modifier code.
fn modified_sequence(number: u8, code: u8, final_byte: char) -> Vec<u8> {
    format!("\x1b[{number};{code}{final_byte}").into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn character(s: &str) -> TerminalKey {
        TerminalKey::Character(KeyText::new(s).expect("test text must be non-empty"))
    }

    fn no_mods() -> TerminalModifiers {
        TerminalModifiers::default()
    }

    fn ctrl() -> TerminalModifiers {
        TerminalModifiers {
            ctrl: true,
            ..Default::default()
        }
    }

    fn alt() -> TerminalModifiers {
        TerminalModifiers {
            alt: true,
            ..Default::default()
        }
    }

    fn modes(app_cursor: bool) -> KeyEncodingModes {
        KeyEncodingModes {
            app_cursor,
            kitty: KittyKeyboardFlags::empty(),
        }
    }

    fn kitty_modes(app_cursor: bool) -> KeyEncodingModes {
        KeyEncodingModes {
            app_cursor,
            kitty: KittyKeyboardFlags::DISAMBIGUATE,
        }
    }

    /// Asserts that Enter sends a carriage return.
    ///
    /// Case: the user submits a shell command line with Enter.
    #[test]
    fn enter_is_carriage_return() {
        assert_eq!(
            encode_key(&TerminalKey::Enter, &no_mods(), modes(false)),
            vec![0x0d]
        );
    }

    /// Asserts that Backspace sends DEL.
    ///
    /// Case: the user erases the character before the cursor at a shell prompt.
    #[test]
    fn backspace_is_del() {
        assert_eq!(
            encode_key(&TerminalKey::Backspace, &no_mods(), modes(false)),
            vec![0x7f]
        );
    }

    /// Asserts that Tab sends HT.
    ///
    /// Case: the user presses Tab to complete a path at a shell prompt.
    #[test]
    fn tab_is_horizontal_tab() {
        assert_eq!(
            encode_key(&TerminalKey::Tab, &no_mods(), modes(false)),
            vec![0x09]
        );
    }

    /// Asserts that Tab with Shift as the only modifier sends the back
    /// tab sequence `CSI Z`, while Tab with any other modifier added still
    /// sends HT.
    ///
    /// Case: the user presses Shift-Tab to move back through the fields
    /// of a form in a full-screen application.
    #[test]
    fn shift_tab_is_the_back_tab_sequence() {
        let shift = TerminalModifiers {
            shift: true,
            ..Default::default()
        };
        assert_eq!(
            encode_key(&TerminalKey::Tab, &shift, modes(false)),
            b"\x1b[Z".to_vec()
        );
        let ctrl_shift = TerminalModifiers {
            ctrl: true,
            shift: true,
            ..Default::default()
        };
        assert_eq!(
            encode_key(&TerminalKey::Tab, &ctrl_shift, modes(false)),
            vec![0x09]
        );
        for other in [
            TerminalModifiers {
                alt: true,
                shift: true,
                ..Default::default()
            },
            TerminalModifiers {
                meta: true,
                shift: true,
                ..Default::default()
            },
        ] {
            assert_eq!(
                encode_key(&TerminalKey::Tab, &other, modes(false)),
                vec![0x09],
                "{other:?} leaves Tab as HT"
            );
        }
    }

    /// Asserts that Escape sends ESC.
    ///
    /// Case: the user leaves insert mode in a full-screen editor with Escape.
    #[test]
    fn escape_is_esc() {
        assert_eq!(
            encode_key(&TerminalKey::Escape, &no_mods(), modes(false)),
            vec![0x1b]
        );
    }

    /// Asserts that the editing keypad keys send the same sequences
    /// whether or not DECCKM is set.
    ///
    /// Case: a full-screen editor turns on application cursor keys, and
    /// the user presses Insert, Delete, PageUp, or PageDown in it.
    #[test]
    fn editing_keypad_ignores_cursor_mode() {
        for app_cursor in [false, true] {
            assert_eq!(
                encode_key(
                    &TerminalKey::Editing(EditingKey::Insert),
                    &no_mods(),
                    modes(app_cursor)
                ),
                b"\x1b[2~".to_vec()
            );
            assert_eq!(
                encode_key(
                    &TerminalKey::Editing(EditingKey::Delete),
                    &no_mods(),
                    modes(app_cursor)
                ),
                b"\x1b[3~".to_vec()
            );
            assert_eq!(
                encode_key(
                    &TerminalKey::Editing(EditingKey::PageUp),
                    &no_mods(),
                    modes(app_cursor)
                ),
                b"\x1b[5~".to_vec()
            );
            assert_eq!(
                encode_key(
                    &TerminalKey::Editing(EditingKey::PageDown),
                    &no_mods(),
                    modes(app_cursor)
                ),
                b"\x1b[6~".to_vec()
            );
        }
    }

    /// Asserts that the arrow keys send `SS3` sequences while DECCKM is set.
    ///
    /// Case: a full-screen pager turns on application cursor keys and the
    /// user scrolls it with the arrow keys.
    #[test]
    fn arrows_use_ss3_in_application_cursor_mode() {
        assert_eq!(
            encode_key(&TerminalKey::Cursor(CursorKey::Up), &no_mods(), modes(true)),
            b"\x1bOA".to_vec()
        );
        assert_eq!(
            encode_key(
                &TerminalKey::Cursor(CursorKey::Down),
                &no_mods(),
                modes(true)
            ),
            b"\x1bOB".to_vec()
        );
        assert_eq!(
            encode_key(
                &TerminalKey::Cursor(CursorKey::Right),
                &no_mods(),
                modes(true)
            ),
            b"\x1bOC".to_vec()
        );
        assert_eq!(
            encode_key(
                &TerminalKey::Cursor(CursorKey::Left),
                &no_mods(),
                modes(true)
            ),
            b"\x1bOD".to_vec()
        );
    }

    /// Asserts that Ctrl with a letter sends the matching C0 byte.
    ///
    /// Case: the user presses Ctrl-C to interrupt a running command.
    #[test]
    fn ctrl_letter_collapses_to_c0_byte() {
        assert_eq!(
            encode_key(&character("a"), &ctrl(), modes(false)),
            vec![0x01]
        );
        assert_eq!(
            encode_key(&character("c"), &ctrl(), modes(false)),
            vec![0x03]
        );
        assert_eq!(
            encode_key(&character("z"), &ctrl(), modes(false)),
            vec![0x1a]
        );
    }

    /// Asserts that Alt with a letter sends the letter prefixed with ESC.
    ///
    /// Case: the user presses Alt-H at a shell prompt that binds it.
    #[test]
    fn alt_letter_is_esc_prefixed() {
        assert_eq!(
            encode_key(&character("h"), &alt(), modes(false)),
            b"\x1bh".to_vec()
        );
    }

    /// Asserts that a key takes the kitty keyboard protocol's form only while
    /// the modes hold flag 1, and that DECCKM leaves that form alone.
    ///
    /// Case: the user presses Shift+Enter at a plain shell prompt and then in
    /// Claude Code, which turned the protocol on, and presses Up in a program
    /// that turned on both the protocol and application cursor keys.
    #[test]
    fn the_kitty_form_needs_flag_1() {
        let shift = TerminalModifiers {
            shift: true,
            ..Default::default()
        };
        assert_eq!(
            encode_key(&TerminalKey::Enter, &shift, modes(false)),
            b"\r".to_vec()
        );
        assert_eq!(
            encode_key(&TerminalKey::Enter, &shift, kitty_modes(false)),
            b"\x1b[13;2u".to_vec()
        );
        assert_eq!(
            encode_key(
                &TerminalKey::Cursor(CursorKey::Up),
                &no_mods(),
                kitty_modes(true)
            ),
            b"\x1b[A".to_vec()
        );
    }

    /// Asserts that a key the layout gave no character for sends no bytes
    /// while no kitty keyboard flag is set.
    ///
    /// Case: on Windows, the user presses Ctrl+Alt+a at a shell prompt on a
    /// layout that gives no character for that chord.
    #[test]
    fn an_unidentified_key_sends_nothing_without_flags() {
        let ctrl_alt = TerminalModifiers {
            ctrl: true,
            alt: true,
            ..Default::default()
        };
        assert!(
            encode_key(
                &TerminalKey::Unidentified { unshifted: 'a' },
                &ctrl_alt,
                modes(false)
            )
            .is_empty()
        );
    }

    /// Asserts that Meta keeps its legacy bytes while no kitty keyboard flag
    /// is set: ESC before text, and nothing added to the cursor keys or
    /// Enter.
    ///
    /// Case: a focused webview forwards Cmd+=, Cmd+Up or Cmd+Enter to a
    /// shell that did not turn on the kitty keyboard protocol.
    #[test]
    fn meta_keeps_its_legacy_bytes_without_flags() {
        let meta = TerminalModifiers {
            meta: true,
            ..Default::default()
        };
        assert_eq!(
            encode_key(&character("="), &meta, modes(false)),
            b"\x1b=".to_vec()
        );
        assert_eq!(
            encode_key(&TerminalKey::Cursor(CursorKey::Up), &meta, modes(false)),
            b"\x1b[A".to_vec()
        );
        assert_eq!(
            encode_key(&TerminalKey::Enter, &meta, modes(false)),
            b"\r".to_vec()
        );
    }
}
