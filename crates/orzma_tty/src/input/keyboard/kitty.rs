//! The kitty keyboard protocol's flag 1 (disambiguate escape codes) form of
//! a key press.

use super::{CursorKey, EditingKey, FunctionKey, TerminalKey, TerminalModifiers};

/// Encodes a key press in the kitty keyboard protocol's flag 1 form.
///
/// Enter, Tab and Backspace with no modifier held send CR, HT and DEL. A
/// text key, and a keypad key that typed text, send the text as typed unless
/// Ctrl, Alt or Meta is held.
///
/// With one of them held, a text key of one character sends
/// `CSI number ; m u` numbered by its un-shifted key, and a longer text keeps
/// its legacy form; a keypad key sends its own keypad number. Every other key
/// sends `CSI number ; m final` with its key number and final byte from the
/// protocol's table.
///
/// `m` is 1 plus Shift 1, Alt 2, Ctrl 4 and Meta 8, the protocol's Super bit;
/// `; m` is left out when no modifier is held, and a number of 1 is then left
/// out too. DECCKM does not affect the form.
///
/// # References
///
/// - <https://sw.kovidgoyal.net/kitty/keyboard-protocol/>, "Disambiguate
///   escape codes": "all key events that do not generate text are
///   represented in one of the following two forms: `CSI number; modifier u`
///   and `CSI 1; modifier [~ABCDEFHPQS]`".
/// - The same page, "Functional key definitions", for each key's number and
///   final byte.
pub(super) fn encode(key: &TerminalKey, mods: &TerminalModifiers) -> Vec<u8> {
    let code = mods.kitty_code();
    let ctrl_alt_or_meta = mods.ctrl || mods.alt || mods.meta;
    let (number, final_byte) = match key {
        TerminalKey::Enter if code.is_none() => return vec![0x0d],
        TerminalKey::Tab if code.is_none() => return vec![0x09],
        TerminalKey::Backspace if code.is_none() => return vec![0x7f],
        TerminalKey::Character(text)
        | TerminalKey::Keypad {
            text: Some(text), ..
        } if !ctrl_alt_or_meta => {
            return text.as_str().as_bytes().to_vec();
        }
        TerminalKey::Character(text) => match text.key_number() {
            Some(number) => (number, 'u'),
            None => return text.encode(mods),
        },
        TerminalKey::Keypad { key, .. } => key.kitty_key(),
        TerminalKey::Unidentified { unshifted } => (u32::from(*unshifted), 'u'),
        TerminalKey::Enter => (13, 'u'),
        TerminalKey::Tab => (9, 'u'),
        TerminalKey::Backspace => (127, 'u'),
        TerminalKey::Escape => (27, 'u'),
        TerminalKey::Cursor(CursorKey::Up) => (1, 'A'),
        TerminalKey::Cursor(CursorKey::Down) => (1, 'B'),
        TerminalKey::Cursor(CursorKey::Right) => (1, 'C'),
        TerminalKey::Cursor(CursorKey::Left) => (1, 'D'),
        TerminalKey::Cursor(CursorKey::Home) => (1, 'H'),
        TerminalKey::Cursor(CursorKey::End) => (1, 'F'),
        TerminalKey::Editing(EditingKey::Insert) => (2, '~'),
        TerminalKey::Editing(EditingKey::Delete) => (3, '~'),
        TerminalKey::Editing(EditingKey::PageUp) => (5, '~'),
        TerminalKey::Editing(EditingKey::PageDown) => (6, '~'),
        TerminalKey::Function(FunctionKey::F1) => (1, 'P'),
        TerminalKey::Function(FunctionKey::F2) => (1, 'Q'),
        TerminalKey::Function(FunctionKey::F3) => (13, '~'),
        TerminalKey::Function(FunctionKey::F4) => (1, 'S'),
        TerminalKey::Function(FunctionKey::F5) => (15, '~'),
        TerminalKey::Function(FunctionKey::F6) => (17, '~'),
        TerminalKey::Function(FunctionKey::F7) => (18, '~'),
        TerminalKey::Function(FunctionKey::F8) => (19, '~'),
        TerminalKey::Function(FunctionKey::F9) => (20, '~'),
        TerminalKey::Function(FunctionKey::F10) => (21, '~'),
        TerminalKey::Function(FunctionKey::F11) => (23, '~'),
        TerminalKey::Function(FunctionKey::F12) => (24, '~'),
    };
    serialize(number, code, final_byte)
}

/// Builds `CSI number ; code final_byte`, leaving out `; code` when `code`
/// is `None`, and the number as well when it is 1 and `code` is `None`.
fn serialize(number: u32, code: Option<u8>, final_byte: char) -> Vec<u8> {
    match (number, code) {
        (_, Some(code)) => format!("\x1b[{number};{code}{final_byte}"),
        (1, None) => format!("\x1b[{final_byte}"),
        (_, None) => format!("\x1b[{number}{final_byte}"),
    }
    .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{KeyText, KeypadKey};

    /// The modifiers `names` lists, where `S`, `A`, `C` and `M` stand for
    /// Shift, Alt, Ctrl and Meta, as in `"C+S"`.
    fn held(names: &str) -> TerminalModifiers {
        TerminalModifiers {
            ctrl: names.contains('C'),
            shift: names.contains('S'),
            alt: names.contains('A'),
            meta: names.contains('M'),
        }
    }

    fn character(text: &str) -> TerminalKey {
        TerminalKey::Character(KeyText::new(text).expect("test text is non-empty"))
    }

    fn character_with_unshifted(text: &str, unshifted: char) -> TerminalKey {
        TerminalKey::Character(
            KeyText::new(text)
                .expect("test text is non-empty")
                .with_unshifted(unshifted),
        )
    }

    fn function(key: FunctionKey) -> TerminalKey {
        TerminalKey::Function(key)
    }

    fn cursor(key: CursorKey) -> TerminalKey {
        TerminalKey::Cursor(key)
    }

    fn editing(key: EditingKey) -> TerminalKey {
        TerminalKey::Editing(key)
    }

    fn keypad(key: KeypadKey) -> TerminalKey {
        TerminalKey::Keypad { key, text: None }
    }

    fn keypad_typing(key: KeypadKey, text: &str) -> TerminalKey {
        TerminalKey::Keypad {
            key,
            text: Some(KeyText::new(text).expect("test text is non-empty")),
        }
    }

    fn assert_cases(cases: &[(TerminalKey, TerminalModifiers, &[u8])]) {
        for (key, mods, expected) in cases {
            assert_eq!(encode(key, mods), *expected, "{key:?} with {mods:?}");
        }
    }

    /// Asserts that Enter, Tab and Backspace with no modifier held keep the
    /// CR, HT and DEL they send in the legacy form.
    ///
    /// Case: a program that turned on the kitty keyboard protocol reads a
    /// line, and the user submits it, completes a word in it, or erases a
    /// character from it.
    #[test]
    fn enter_tab_and_backspace_keep_their_bytes_without_a_modifier() {
        assert_cases(&[
            (TerminalKey::Enter, held(""), b"\r"),
            (TerminalKey::Tab, held(""), b"\t"),
            (TerminalKey::Backspace, held(""), b"\x7f"),
        ]);
    }

    /// Asserts that Enter, Tab and Backspace with a modifier held take the
    /// `CSI number ; m u` form rather than their legacy bytes.
    ///
    /// Case: the user presses Shift+Enter in Claude Code to start a new line
    /// of a prompt, Shift+Tab to switch its mode, or Ctrl+Backspace to erase
    /// a word.
    #[test]
    fn modified_enter_tab_and_backspace_take_the_csi_u_form() {
        assert_cases(&[
            (TerminalKey::Enter, held("S"), b"\x1b[13;2u"),
            (TerminalKey::Enter, held("C"), b"\x1b[13;5u"),
            (TerminalKey::Tab, held("S"), b"\x1b[9;2u"),
            (TerminalKey::Tab, held("A"), b"\x1b[9;3u"),
            (TerminalKey::Backspace, held("C"), b"\x1b[127;5u"),
        ]);
    }

    /// Asserts that Escape takes the `CSI 27 u` form whether or not a
    /// modifier is held, rather than sending a lone ESC.
    ///
    /// Case: the user presses Escape, alone or with Alt, in an editor that
    /// turned on the kitty keyboard protocol.
    #[test]
    fn escape_always_takes_the_csi_u_form() {
        assert_cases(&[
            (TerminalKey::Escape, held(""), b"\x1b[27u"),
            (TerminalKey::Escape, held("A"), b"\x1b[27;3u"),
        ]);
    }

    /// Asserts that a one-character text key with Ctrl, Alt or Meta held
    /// takes the `CSI number ; m u` form, numbered by the key's lowercase
    /// character, rather than a control byte or an ESC prefix.
    ///
    /// Case: in a program that turned on the kitty keyboard protocol, the
    /// user presses Ctrl+C to interrupt it, Alt+a or Ctrl+Alt+a for a binding,
    /// Alt+[ in an editor, or Ctrl+Space to set the mark.
    #[test]
    fn text_with_ctrl_alt_or_meta_takes_the_csi_u_form() {
        assert_cases(&[
            (character("c"), held("C"), b"\x1b[99;5u"),
            (character("A"), held("C+S"), b"\x1b[97;6u"),
            (character("a"), held("A"), b"\x1b[97;3u"),
            (character("A"), held("S+A"), b"\x1b[97;4u"),
            (character("a"), held("C+A"), b"\x1b[97;7u"),
            (character("["), held("A"), b"\x1b[91;3u"),
            (character(" "), held("C"), b"\x1b[32;5u"),
        ]);
    }

    /// Asserts that a text key with no modifier or with Shift alone is sent
    /// as the UTF-8 of the text it typed.
    ///
    /// Case: the user types a word, a capital letter, or an accented letter
    /// into a program that turned on the kitty keyboard protocol.
    #[test]
    fn text_alone_or_with_shift_is_sent_as_typed() {
        assert_cases(&[
            (character("a"), held(""), b"a"),
            (character("A"), held("S"), b"A"),
            (character("é"), held(""), "é".as_bytes()),
        ]);
    }

    /// Asserts that a text key is numbered by the un-shifted key the host
    /// recorded, and without one by its character lowercased when that
    /// lowercase is one character, or by the character itself otherwise.
    ///
    /// Case: on a US layout the user presses Shift+Alt+1, which types `!`,
    /// on a German layout Ctrl+Shift+Ä or Ctrl+ß, and on a Turkish layout
    /// Ctrl+Shift+İ, in a program that turned on the kitty keyboard protocol.
    #[test]
    fn the_key_number_is_the_unshifted_key() {
        assert_cases(&[
            (
                character_with_unshifted("!", '1'),
                held("S+A"),
                b"\x1b[49;4u",
            ),
            (character("!"), held("S+A"), b"\x1b[33;4u"),
            (character("Ä"), held("C+S"), b"\x1b[228;6u"),
            (character("ß"), held("C"), b"\x1b[223;5u"),
            (character("İ"), held("C+S"), b"\x1b[304;6u"),
        ]);
    }

    /// Asserts that text of more than one character keeps its legacy form,
    /// sent as is or after ESC when Alt is held, whatever modifier is held.
    ///
    /// Case: an input method commits two characters at once while the user
    /// still holds Ctrl or Alt.
    #[test]
    fn multi_character_text_keeps_the_legacy_form() {
        assert_cases(&[
            (character("ab"), held(""), b"ab"),
            (character("ab"), held("C"), b"ab"),
            (character("ab"), held("A"), b"\x1bab"),
        ]);
    }

    /// Asserts that the cursor keys send `CSI final`, dropping the number 1
    /// when no modifier is held, and `CSI 1 ; m final` with one.
    ///
    /// Case: the user moves through a prompt with the arrows, Home and End,
    /// and jumps back a word with Ctrl+Left, in a program that turned on the
    /// kitty keyboard protocol.
    #[test]
    fn cursor_keys_ignore_decckm_and_drop_a_lone_1() {
        assert_cases(&[
            (cursor(CursorKey::Up), held(""), b"\x1b[A"),
            (cursor(CursorKey::Home), held(""), b"\x1b[H"),
            (cursor(CursorKey::End), held(""), b"\x1b[F"),
            (cursor(CursorKey::Left), held("C"), b"\x1b[1;5D"),
        ]);
    }

    /// Asserts that the editing keys send `CSI number ~`, or
    /// `CSI number ; m ~` with a modifier held.
    ///
    /// Case: the user presses Insert, or Ctrl+Delete to delete the next word,
    /// in an editor that turned on the kitty keyboard protocol.
    #[test]
    fn editing_keys_use_the_tilde_form() {
        assert_cases(&[
            (editing(EditingKey::Insert), held(""), b"\x1b[2~"),
            (editing(EditingKey::Delete), held("C"), b"\x1b[3;5~"),
        ]);
    }

    /// Asserts that F1, F2 and F4 send `CSI P`, `CSI Q` and `CSI S` rather
    /// than SS3, that F3 sends `CSI 13 ~`, and that F5 to F12 keep their
    /// tilde form.
    ///
    /// Case: the user presses F1 for help, Shift+F1 or Shift+F3 for a
    /// binding, F5 to refresh, or Ctrl+F12 in a full-screen program that
    /// turned on the kitty keyboard protocol.
    #[test]
    fn function_keys_take_their_flag_1_forms() {
        assert_cases(&[
            (function(FunctionKey::F1), held(""), b"\x1b[P"),
            (function(FunctionKey::F2), held(""), b"\x1b[Q"),
            (function(FunctionKey::F3), held(""), b"\x1b[13~"),
            (function(FunctionKey::F4), held(""), b"\x1b[S"),
            (function(FunctionKey::F1), held("S"), b"\x1b[1;2P"),
            (function(FunctionKey::F3), held("S"), b"\x1b[13;2~"),
            (function(FunctionKey::F5), held(""), b"\x1b[15~"),
            (function(FunctionKey::F12), held("C"), b"\x1b[24;5~"),
        ]);
    }

    /// Asserts that Meta counts toward the modifier parameter as Super,
    /// adding 8, and makes a text key take the `CSI number ; m u` form.
    ///
    /// Case: a focused webview forwards Cmd+=, Cmd+Up or Cmd+Enter to a
    /// program that turned on the kitty keyboard protocol.
    #[test]
    fn meta_counts_as_super() {
        assert_cases(&[
            (character("="), held("M"), b"\x1b[61;9u"),
            (cursor(CursorKey::Up), held("M"), b"\x1b[1;9A"),
            (TerminalKey::Enter, held("M"), b"\x1b[13;9u"),
        ]);
    }

    /// Asserts that a keypad key that typed text sends that text unless
    /// Ctrl, Alt or Meta is held, and that every other keypad key press
    /// sends the key's own kitty number, with Begin as `CSI E`.
    ///
    /// Case: the user types a digit on the keypad, alone, with Shift, with
    /// Ctrl or with Cmd, then presses the keypad's Enter, its arrows with
    /// Num Lock off, Ctrl+Delete, and its centre key, in a program that
    /// turned on the kitty keyboard protocol.
    #[test]
    fn keypad_keys_take_their_kitty_numbers() {
        assert_cases(&[
            (keypad_typing(KeypadKey::One, "1"), held(""), b"1"),
            (keypad_typing(KeypadKey::One, "1"), held("S"), b"1"),
            (
                keypad_typing(KeypadKey::One, "1"),
                held("C"),
                b"\x1b[57400;5u",
            ),
            (
                keypad_typing(KeypadKey::One, "1"),
                held("M"),
                b"\x1b[57400;9u",
            ),
            (keypad(KeypadKey::Enter), held(""), b"\x1b[57414u"),
            (keypad(KeypadKey::Up), held(""), b"\x1b[57419u"),
            (keypad(KeypadKey::Delete), held("C"), b"\x1b[57426;5u"),
            (keypad(KeypadKey::Begin), held(""), b"\x1b[E"),
            (keypad(KeypadKey::Begin), held("C"), b"\x1b[1;5E"),
        ]);
    }

    /// Asserts that a key the layout gave no character for is numbered by
    /// the character its key types with no modifier held.
    ///
    /// Case: on Windows, the user presses Ctrl+Alt+a on a layout that gives
    /// no character for that chord, in a program that turned on the kitty
    /// keyboard protocol.
    #[test]
    fn an_unidentified_key_is_keyed_by_its_unshifted_character() {
        assert_cases(&[(
            TerminalKey::Unidentified { unshifted: 'a' },
            held("C+A"),
            b"\x1b[97;7u",
        )]);
    }

    /// Asserts the number and final byte of every key the table maps, sent
    /// with no modifier and with Ctrl.
    ///
    /// Case: the user presses each function key, cursor key, editing key
    /// and keypad key, alone and with Ctrl, in a program that turned on the
    /// kitty keyboard protocol.
    #[test]
    fn every_table_row_is_pinned() {
        let rows: [(TerminalKey, &[u8], &[u8]); 51] = [
            (function(FunctionKey::F1), b"\x1b[P", b"\x1b[1;5P"),
            (function(FunctionKey::F2), b"\x1b[Q", b"\x1b[1;5Q"),
            (function(FunctionKey::F3), b"\x1b[13~", b"\x1b[13;5~"),
            (function(FunctionKey::F4), b"\x1b[S", b"\x1b[1;5S"),
            (function(FunctionKey::F5), b"\x1b[15~", b"\x1b[15;5~"),
            (function(FunctionKey::F6), b"\x1b[17~", b"\x1b[17;5~"),
            (function(FunctionKey::F7), b"\x1b[18~", b"\x1b[18;5~"),
            (function(FunctionKey::F8), b"\x1b[19~", b"\x1b[19;5~"),
            (function(FunctionKey::F9), b"\x1b[20~", b"\x1b[20;5~"),
            (function(FunctionKey::F10), b"\x1b[21~", b"\x1b[21;5~"),
            (function(FunctionKey::F11), b"\x1b[23~", b"\x1b[23;5~"),
            (function(FunctionKey::F12), b"\x1b[24~", b"\x1b[24;5~"),
            (cursor(CursorKey::Up), b"\x1b[A", b"\x1b[1;5A"),
            (cursor(CursorKey::Down), b"\x1b[B", b"\x1b[1;5B"),
            (cursor(CursorKey::Right), b"\x1b[C", b"\x1b[1;5C"),
            (cursor(CursorKey::Left), b"\x1b[D", b"\x1b[1;5D"),
            (cursor(CursorKey::Home), b"\x1b[H", b"\x1b[1;5H"),
            (cursor(CursorKey::End), b"\x1b[F", b"\x1b[1;5F"),
            (editing(EditingKey::Insert), b"\x1b[2~", b"\x1b[2;5~"),
            (editing(EditingKey::Delete), b"\x1b[3~", b"\x1b[3;5~"),
            (editing(EditingKey::PageUp), b"\x1b[5~", b"\x1b[5;5~"),
            (editing(EditingKey::PageDown), b"\x1b[6~", b"\x1b[6;5~"),
            (keypad(KeypadKey::Zero), b"\x1b[57399u", b"\x1b[57399;5u"),
            (keypad(KeypadKey::One), b"\x1b[57400u", b"\x1b[57400;5u"),
            (keypad(KeypadKey::Two), b"\x1b[57401u", b"\x1b[57401;5u"),
            (keypad(KeypadKey::Three), b"\x1b[57402u", b"\x1b[57402;5u"),
            (keypad(KeypadKey::Four), b"\x1b[57403u", b"\x1b[57403;5u"),
            (keypad(KeypadKey::Five), b"\x1b[57404u", b"\x1b[57404;5u"),
            (keypad(KeypadKey::Six), b"\x1b[57405u", b"\x1b[57405;5u"),
            (keypad(KeypadKey::Seven), b"\x1b[57406u", b"\x1b[57406;5u"),
            (keypad(KeypadKey::Eight), b"\x1b[57407u", b"\x1b[57407;5u"),
            (keypad(KeypadKey::Nine), b"\x1b[57408u", b"\x1b[57408;5u"),
            (keypad(KeypadKey::Decimal), b"\x1b[57409u", b"\x1b[57409;5u"),
            (keypad(KeypadKey::Divide), b"\x1b[57410u", b"\x1b[57410;5u"),
            (
                keypad(KeypadKey::Multiply),
                b"\x1b[57411u",
                b"\x1b[57411;5u",
            ),
            (
                keypad(KeypadKey::Subtract),
                b"\x1b[57412u",
                b"\x1b[57412;5u",
            ),
            (keypad(KeypadKey::Add), b"\x1b[57413u", b"\x1b[57413;5u"),
            (keypad(KeypadKey::Enter), b"\x1b[57414u", b"\x1b[57414;5u"),
            (keypad(KeypadKey::Equal), b"\x1b[57415u", b"\x1b[57415;5u"),
            (
                keypad(KeypadKey::Separator),
                b"\x1b[57416u",
                b"\x1b[57416;5u",
            ),
            (keypad(KeypadKey::Left), b"\x1b[57417u", b"\x1b[57417;5u"),
            (keypad(KeypadKey::Right), b"\x1b[57418u", b"\x1b[57418;5u"),
            (keypad(KeypadKey::Up), b"\x1b[57419u", b"\x1b[57419;5u"),
            (keypad(KeypadKey::Down), b"\x1b[57420u", b"\x1b[57420;5u"),
            (keypad(KeypadKey::PageUp), b"\x1b[57421u", b"\x1b[57421;5u"),
            (
                keypad(KeypadKey::PageDown),
                b"\x1b[57422u",
                b"\x1b[57422;5u",
            ),
            (keypad(KeypadKey::Home), b"\x1b[57423u", b"\x1b[57423;5u"),
            (keypad(KeypadKey::End), b"\x1b[57424u", b"\x1b[57424;5u"),
            (keypad(KeypadKey::Insert), b"\x1b[57425u", b"\x1b[57425;5u"),
            (keypad(KeypadKey::Delete), b"\x1b[57426u", b"\x1b[57426;5u"),
            (keypad(KeypadKey::Begin), b"\x1b[E", b"\x1b[1;5E"),
        ];
        for (key, unmodified, with_ctrl) in rows {
            assert_eq!(encode(&key, &held("")), unmodified, "{key:?}");
            assert_eq!(encode(&key, &held("C")), with_ctrl, "{key:?} with Ctrl");
        }
    }
}
