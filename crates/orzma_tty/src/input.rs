//! Encoders that turn user input events into the byte sequences written to
//! the PTY.

use orzma_vt::prelude::{MouseEncoding, VtModes};

mod keyboard;
mod mouse;
mod paste;
mod wheel;

pub use keyboard::*;
pub use mouse::*;

/// VT-encoded bytes bound for the PTY, produced by a user input event.
///
/// Covers escape sequences, C0 control bytes, and plain UTF-8 alike —
/// anything the terminal writes into the PTY on the user's behalf.
pub struct PtyInput(Vec<u8>);

impl PtyInput {
    /// Encodes a key press. Total — every `TerminalKey` has a representation.
    ///
    /// # References
    ///
    /// - [PC-Style Function Keys] — cursor keys (arrows plus Home/End) under
    ///   DECCKM: `CSI A/B/C/D/H/F` in normal mode, `SS3 A/B/C/D/H/F` in
    ///   application mode.
    /// - [VT220-Style Function Keys] — the 6-key editing keypad
    ///   (`CSI 2 ~`, `CSI 3 ~`, `CSI 5 ~`, `CSI 6 ~`), explicitly unaffected
    ///   by DECCKM.
    /// - [Alt and Meta Keys] — `metaSendsEscape` / `altSendsEscape`: prefix the
    ///   key with `ESC`.
    ///
    /// [PC-Style Function Keys]: https://invisible-island.net/xterm/ctlseqs/ctlseqs.html#h3-PC-Style-Function-Keys
    /// [VT220-Style Function Keys]: https://invisible-island.net/xterm/ctlseqs/ctlseqs.html#h3-VT220-Style-Function-Keys
    /// [Alt and Meta Keys]: https://invisible-island.net/xterm/ctlseqs/ctlseqs.html#h3-Alt-and-Meta-Keys
    pub fn encode_key(key: &TerminalKey, mods: &TerminalModifiers, modes: VtModes) -> Self {
        Self(keyboard::encode_key(
            key,
            mods,
            modes.app_cursor,
            modes.keypad_mode,
        ))
    }

    /// Encodes one mouse report in the given mouse encoding. UTF-8
    /// (1005) is unimplemented and routed to X10 framing.
    ///
    /// # References
    ///
    /// - [Mouse Tracking] — button/modifier/motion `cb` packing and the
    ///   X10 vs SGR report framing.
    ///
    /// [Mouse Tracking]: https://invisible-island.net/xterm/ctlseqs/ctlseqs.html#h2-Mouse-Tracking
    pub fn encode_mouse(report: &MouseReport, encoding: MouseEncoding) -> Self {
        Self(report.encode(encoding))
    }

    /// Encodes a paste of clipboard text, honouring bracketed-paste mode
    /// (DECSET 2004).
    ///
    /// - `bracketed = true`: strips every embedded occurrence of the four
    ///   bracketed-paste marker forms (7-bit `ESC [ 200~` / `ESC [ 201~` and
    ///   C1 `U+009B 200~` / `U+009B 201~`) in a fixed-point loop, then wraps
    ///   the sanitized body in `ESC [ 200 ~` ... `ESC [ 201 ~`; the body is
    ///   otherwise passed through byte-for-byte. Closes the paste-injection
    ///   class documented in kitty commit 668f6fa and Alacritty issue #800.
    /// - `bracketed = false`: normalizes line endings (`\r\n` and lone `\n`
    ///   become `\r`) and filters nothing else — an unbracketed paste has
    ///   the same authority as typed input.
    ///
    /// The input domain is UTF-8 `&str`: the C1 markers match the codepoint
    /// `U+009B` (UTF-8 `C2 9B`); a raw `9B` byte is not representable and
    /// out of scope.
    ///
    /// # References
    ///
    /// - [Bracketed Paste Mode] — DECSET 2004 and the `ESC [ 200 ~` /
    ///   `ESC [ 201 ~` framing.
    ///
    /// [Bracketed Paste Mode]: https://invisible-island.net/xterm/ctlseqs/ctlseqs.html#h2-Bracketed-Paste-Mode
    pub fn encode_paste(text: &str, bracketed: bool) -> Self {
        Self(paste::encode_paste(text, bracketed))
    }

    /// Returns the encoded bytes, ready to write to the PTY.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::mouse::{
        CellCoord, MouseButton, MouseReport, MouseReportKind, ProtocolModifiers,
    };

    #[test]
    fn encode_mouse_wraps_report_bytes() {
        let report = MouseReport {
            button: MouseButton::Left,
            kind: MouseReportKind::Press,
            cell: CellCoord { col: 5, row: 7 },
            mods: ProtocolModifiers::default(),
        };
        let input = PtyInput::encode_mouse(&report, MouseEncoding::Sgr);
        assert_eq!(input.as_bytes(), b"\x1b[<0;5;7M");
    }
}
