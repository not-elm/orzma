//! Tests for how the interpreter reports the operating system command
//! that writes the system clipboard.

use super::*;

/// Asserts that a clipboard command naming `c` reports the text its
/// payload decodes to.
///
/// Case: a user yanks a line into Neovim's `+` register, and the
/// editor's osc52 provider hands the text to the terminal over the
/// PTY because the session is remote.
#[test]
fn a_clipboard_command_reports_the_decoded_text() {
    let (_device, output) = interpret_fully(b"\x1b]52;c;aGk=\x07");
    assert_eq!(
        output.signals,
        vec![VtSignal::Clipboard {
            content: "hi".to_owned()
        }]
    );
}

/// Asserts that a payload carrying a `;` is refused whole as not
/// base64, rather than read up to the first `;`.
///
/// Case: a script joins two encoded strings with `;`, so the payload
/// runs on past the first one.
#[test]
fn a_payload_carrying_a_semicolon_reports_nothing() {
    let (_device, output) = interpret_fully(b"\x1b]52;c;aGk=;Zm9v\x07");
    assert!(output.signals.is_empty());
}

/// Asserts that a clipboard command closed by the string terminator
/// reports the same text as one closed by BEL.
///
/// Case: a program follows the eight-bit conventions and ends its
/// operating system commands with `ESC \` throughout.
#[test]
fn a_clipboard_command_closed_by_the_string_terminator_reports_the_text() {
    let (_device, output) = interpret_fully(b"\x1b]52;c;aGk=\x1b\\");
    assert_eq!(
        output.signals,
        vec![VtSignal::Clipboard {
            content: "hi".to_owned()
        }]
    );
}

/// Asserts that writing the clipboard leaves the chunk undamaged.
///
/// Case: an editor copies a yank to the clipboard without drawing
/// anything, and the terminal has nothing to repaint.
#[test]
fn a_clipboard_command_leaves_the_chunk_undamaged() {
    assert!(!damage_of(b"\x1b]52;c;aGk=\x07"));
}
