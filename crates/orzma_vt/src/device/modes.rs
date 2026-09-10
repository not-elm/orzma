//! The DECSET / DECRST modes the device carries and the enums they
//! select among.

/// Snapshot of the device-wide DECSET / DECRST modes.
///
/// # References
///
/// - [XTerm Control Sequences] — `CSI ? Pm h` (DEC Private Mode Set,
///   DECSET); each field cites its DECSET number.
/// - [Mouse Tracking] — the reporting and coordinate-encoding modes
///   carried by [`MouseTracking`] and [`MouseEncoding`].
///
/// [XTerm Control Sequences]: https://invisible-island.net/xterm/ctlseqs/ctlseqs.html
/// [Mouse Tracking]: https://invisible-island.net/xterm/ctlseqs/ctlseqs.html#h2-Mouse-Tracking
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VtModes {
    /// DECSET 1049/47: which of the two screens the device shows.
    ///
    /// This is the only record of the active screen — the screen pair
    /// itself is pure storage and keeps no such flag, so the two
    /// cannot disagree.
    pub active_screen: ScreenKind,
    /// DECCKM (DECSET 1): arrow keys send SS3 instead of CSI.
    pub app_cursor: bool,
    /// The mode selects whether the numeric keypad sends ASCII numerals or application function.
    pub keypad_mode: KeypadMode,
    /// DECSET 2004: pastes are wrapped in `ESC[200~` / `ESC[201~`.
    pub bracketed_paste: bool,
    /// DECSET 1007: enables alternate-scroll translation.
    ///
    /// This stores the mode itself, which is not actionable on its own
    /// — it takes effect only while [`Self::active_screen`] is
    /// [`ScreenKind::Alternate`]. Read it through
    /// [`Self::alternate_scroll_active`] rather than on its own.
    pub alternate_scroll: bool,
    /// DECSET 1004: the app wants `CSI I` / `CSI O` focus reports.
    pub focus_in_out: bool,
    /// DECTCEM (DECSET 25): whether the text cursor is drawn.
    ///
    /// The device carries this rather than either screen, so a switch to
    /// the alternate screen keeps the state the application set. DECSC
    /// does not save it either — the VT510 saved-item list does not name
    /// cursor visibility.
    pub text_cursor_enable: TextCursorEnable,
    /// Coordinate encoding for mouse reports.
    pub mouse_encoding: MouseEncoding,
    /// Which mouse events the app asked to receive.
    pub mouse_tracking: MouseTracking,
}

impl VtModes {
    /// Whether alternate-scroll translation is in effect: DECSET 1007
    /// set *and* the alternate screen shown.
    ///
    /// This is not "the wheel sends arrow keys" — an active mouse
    /// tracking mode outranks alternate scroll, and resolving that
    /// order is the host's wheel routing, not this snapshot's.
    pub const fn alternate_scroll_active(&self) -> bool {
        matches!(self.active_screen, ScreenKind::Alternate) && self.alternate_scroll
    }
}

/// Whether DECTCEM (DECSET 25) has the text cursor enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextCursorEnable {
    /// `CSI ? 25 h`: the cursor is drawn. The power-up default.
    #[default]
    Shown,
    /// `CSI ? 25 l`: the cursor is not drawn.
    Hidden,
}

impl TextCursorEnable {
    /// The state `DECSET 25` selects when set and `DECRST 25` when reset.
    pub fn from_decset(enabled: bool) -> Self {
        if enabled { Self::Shown } else { Self::Hidden }
    }
}

/// The mode selects whether the numeric keypad sends ASCII numerals or application function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KeypadMode {
    #[default]
    Numeric,
    Application,
}

impl KeypadMode {
    /// The mode `DECSET 66` selects when set and `DECRST 66` when reset.
    pub fn from_decset(enabled: bool) -> Self {
        if enabled {
            Self::Application
        } else {
            Self::Numeric
        }
    }
}

/// Which of a device's two screens is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScreenKind {
    /// The scrollback-backed screen a shell writes to.
    #[default]
    Primary,
    /// The scrollback-free screen full-screen applications take over.
    Alternate,
}

impl ScreenKind {
    /// The screen `DECSET 47` shows when set and `DECRST 47` when reset.
    pub fn from_decset(enabled: bool) -> Self {
        if enabled {
            Self::Alternate
        } else {
            Self::Primary
        }
    }
}

/// Mouse-report coordinate encoding.
///
/// The encodings are mutually exclusive: xterm keeps DECSET 1005/1006
/// as separate numbers, but setting one replaces the other.
///
/// # References
///
/// - [Mouse Tracking] — the "Extended coordinates" prose defines
///   DECSET 1005 (UTF-8) and DECSET 1006 (SGR) as extensions of the
///   default single-byte encoding.
///
/// [Mouse Tracking]: https://invisible-island.net/xterm/ctlseqs/ctlseqs.html#h2-Mouse-Tracking
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseEncoding {
    /// Default byte-triplet encoding (coordinates capped at 223).
    #[default]
    X10,
    /// DECSET 1005: UTF-8 coordinate extension.
    Utf8,
    /// DECSET 1006: SGR extended reports (`CSI < … M/m`).
    Sgr,
}

impl MouseEncoding {
    /// The encoding `mode` selects, applied to the current one; `None`
    /// when the number names no encoding this terminal answers.
    ///
    /// A `DECRST` returns to [`Self::X10`] only when the number names
    /// the encoding currently in force. The variants and the numbers
    /// correspond one to one, so an application that resets an encoding
    /// it never set would otherwise disable the one it did.
    // TODO: Answer DECSET 1005 here once `MouseReport::encode` really
    // implements the UTF-8 coordinate extension. Selecting `Utf8` while
    // the encoder falls back to X10 would advertise a protocol whose
    // reports go wrong past column 95.
    pub fn with_decset(self, mode: u16, enabled: bool) -> Option<Self> {
        let encoding = match mode {
            1006 => Self::Sgr,
            _ => return None,
        };
        Some(match (enabled, self == encoding) {
            (true, _) => encoding,
            (false, true) => Self::X10,
            (false, false) => self,
        })
    }
}

/// Mouse-tracking level.
///
/// The levels are mutually exclusive: each DECSET below replaces the
/// currently active level.
///
/// # References
///
/// - [Mouse Tracking] — protocol overview; DECSET 1000 enables
///   press/release ("normal") tracking.
/// - [Button-event tracking] — DECSET 1002: presses/releases plus
///   motion while a button is held.
/// - [Any-event tracking] — DECSET 1003: all motion, regardless of
///   button state.
///
/// [Mouse Tracking]: https://invisible-island.net/xterm/ctlseqs/ctlseqs.html#h2-Mouse-Tracking
/// [Button-event tracking]: https://invisible-island.net/xterm/ctlseqs/ctlseqs.html#h3-Button-event-tracking
/// [Any-event tracking]: https://invisible-island.net/xterm/ctlseqs/ctlseqs.html#h3-Any-event-tracking
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseTracking {
    /// No mouse reporting.
    #[default]
    Off,
    /// DECSET 1000: button press/release only ("vt200" tracking).
    Clicks,
    /// DECSET 1002: clicks plus drag motion.
    Drag,
    /// DECSET 1003: all motion.
    Motion,
}

impl MouseTracking {
    /// The level `mode` selects, applied to the current one; `None`
    /// when the number names no tracking level.
    ///
    /// A `DECSET` replaces the level outright. A `DECRST` clears it only
    /// when the number names the level currently in force, for the same
    /// reason [`MouseEncoding::with_decset`] records.
    pub fn with_decset(self, mode: u16, enabled: bool) -> Option<Self> {
        let level = match mode {
            1000 => Self::Clicks,
            1002 => Self::Drag,
            1003 => Self::Motion,
            _ => return None,
        };
        Some(match (enabled, self == level) {
            (true, _) => level,
            (false, true) => Self::Off,
            (false, false) => self,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts that a device that has seen no DECTCEM starts with the
    /// cursor shown, which is the mode's documented default.
    ///
    /// Case: a terminal is spawned and the shell prints its first
    /// prompt before any application has touched cursor visibility.
    #[test]
    fn the_text_cursor_starts_shown() {
        assert_eq!(
            VtModes::default().text_cursor_enable,
            TextCursorEnable::Shown
        );
    }

    /// Asserts that `DECSET 25` selects `Shown` and `DECRST 25`
    /// selects `Hidden`.
    ///
    /// Case: a full-screen editor hides the caret before a repaint and
    /// asks for it back when the repaint is done.
    #[test]
    fn decset_twenty_five_selects_shown_and_decrst_selects_hidden() {
        assert_eq!(TextCursorEnable::from_decset(true), TextCursorEnable::Shown);
        assert_eq!(
            TextCursorEnable::from_decset(false),
            TextCursorEnable::Hidden
        );
    }
}
