//! The modes the device carries — DEC private (DECSET / DECRST) and
//! ANSI (SM / RM) alike — and the enums they select among.

/// Snapshot of the modes the device owns, as opposed to those a screen
/// owns.
///
/// Both screens share every mode here. The ones a screen owns — the
/// cursor origin, the tab stops, the character set mapping — live on the
/// screen itself instead.
///
/// # References
///
/// - [XTerm Control Sequences] — `CSI ? Pm h` (DEC Private Mode Set,
///   DECSET) and `CSI Pm h` (Set Mode, SM); each field cites its number.
/// - [Mouse Tracking] — the reporting and coordinate-encoding modes
///   carried by [`MouseTracking`] and [`MouseEncoding`].
///
/// [XTerm Control Sequences]: https://invisible-island.net/xterm/ctlseqs/ctlseqs.html
/// [Mouse Tracking]: https://invisible-island.net/xterm/ctlseqs/ctlseqs.html#h2-Mouse-Tracking
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VtModes {
    /// DECSET 1049/47: which of the two screens the device shows.
    pub active_screen: ScreenKind,
    /// IRM (`SM 4`): whether a printed character inserts or replaces.
    pub insert_replace: InsertReplaceMode,
    /// DECAWM: whether a graphic character at the right
    /// border wraps to the next line or replaces the last column.
    pub auto_wrap: AutoWrap,
    /// DECCKM (DECSET 1): arrow keys send SS3 instead of CSI.
    pub app_cursor: bool,
    /// The mode selects whether the numeric keypad sends ASCII numerals or application function.
    pub keypad_mode: KeypadMode,
    /// DECSET 2004: pastes are wrapped in `ESC[200~` / `ESC[201~`.
    pub bracketed_paste: bool,
    /// DECSET 1007: enables alternate-scroll translation.
    ///
    /// It takes effect only while [`Self::active_screen`] is
    /// [`ScreenKind::Alternate`], so a reader must go through
    /// [`Self::alternate_scroll_active`] rather than read it on its own.
    pub alternate_scroll: bool,
    /// DECSET 1004: the app wants `CSI I` / `CSI O` focus reports.
    pub focus_in_out: bool,
    /// How the text cursor is presented: its visibility, shape, and
    /// blink.
    ///
    /// A switch to the alternate screen keeps the state the application
    /// set, and DECSC does not save it either.
    pub text_cursor: TextCursorModes,
    /// Coordinate encoding for mouse reports.
    pub mouse_encoding: MouseEncoding,
    /// Which mouse events the app asked to receive.
    pub mouse_tracking: MouseTracking,
}

impl VtModes {
    /// Whether alternate-scroll translation is in effect: DECSET 1007
    /// set *and* the alternate screen shown.
    ///
    /// This is not "the wheel sends arrow keys": an active mouse
    /// tracking mode outranks alternate scroll, and the caller must
    /// resolve that order itself.
    pub const fn alternate_scroll_active(&self) -> bool {
        matches!(self.active_screen, ScreenKind::Alternate) && self.alternate_scroll
    }
}

/// Whether a printed character replaces the cell at the cursor or pushes
/// the rest of the row right.
///
/// Both screens share one value, so an alternate-screen flip shows the
/// mode it left, and `DECSC` does not carry it either.
///
/// # Control Functions
///
/// - `IRM` (`CSI 4 h` / `CSI 4 l`)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InsertReplaceMode {
    /// A printed character overwrites the cell at the cursor.
    #[default]
    Replace,
    /// A printed character pushes the cells at and right of the cursor
    /// one column right, dropping the one past the last column.
    Insert,
}

impl InsertReplaceMode {
    /// The mode `SM 4` selects when set and `RM 4` when reset.
    pub fn from_sm(enabled: bool) -> Self {
        if enabled { Self::Insert } else { Self::Replace }
    }
}

/// Whether a graphic character received at the right border wraps to
/// the next line or replaces the character already in the last column.
///
/// Both screens share one value, and `DECSC` does not carry it.
///
/// The default is [`Self::Enabled`], the set rather than the reset
/// state.
///
/// # Control Functions
///
/// - `DECAWM` (`CSI ? 7 h` / `CSI ? 7 l`)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AutoWrap {
    /// A character at the right border moves to the start of the next
    /// line, scrolling when the cursor is at the end of the scrolling
    /// region.
    #[default]
    Enabled,
    /// A character at the right border replaces the one in the last
    /// column, and the cursor stays there.
    Disabled,
}

impl AutoWrap {
    /// The mode `DECSET 7` selects when set and `DECRST 7` when reset.
    pub fn from_decset(enabled: bool) -> Self {
        if enabled {
            Self::Enabled
        } else {
            Self::Disabled
        }
    }

    /// Whether a character at the right border wraps.
    pub const fn wraps(self) -> bool {
        matches!(self, Self::Enabled)
    }
}

/// Whether the text cursor is drawn.
///
/// Both screens share one value, so an alternate-screen flip shows the
/// mode it left, and `DECSC` does not carry it either.
///
/// # Control Functions
///
/// - `DECTCEM` (`CSI ? 25 h` / `CSI ? 25 l`)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextCursorEnable {
    /// The cursor is drawn; this is the power-up default.
    #[default]
    Shown,
    /// The cursor is not drawn.
    Hidden,
}

impl TextCursorEnable {
    /// The state `DECSET 25` selects when set and `DECRST 25` when reset.
    pub fn from_decset(enabled: bool) -> Self {
        if enabled { Self::Shown } else { Self::Hidden }
    }
}

/// The shape the text cursor is drawn with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorShape {
    /// A block filling the cell; this is the power-up default.
    #[default]
    Block,
    /// A line along the bottom of the cell.
    Underline,
    /// A vertical line at the left of the cell.
    Bar,
}

/// Whether the text cursor blinks or is drawn continuously.
///
/// Both screens share one value, and `DECSC` does not carry it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorBlink {
    /// The cursor is drawn continuously; this is the power-up default.
    #[default]
    Steady,
    /// The cursor alternates between drawn and not drawn.
    Blinking,
}

/// How the text cursor is presented.
///
/// Both screens share one value, and `DECSC` does not carry it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TextCursorModes {
    /// Whether the cursor is drawn at all.
    pub enable: TextCursorEnable,
    /// The shape it takes.
    pub shape: CursorShape,
    /// Whether it blinks.
    pub blink: CursorBlink,
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
/// The encodings are mutually exclusive. DECSET 1005 is not answered.
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
    /// the encoding currently in force.
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
    /// when the number names the level currently in force.
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

    /// Asserts that a device that has seen no DECAWM starts with
    /// autowrap enabled, which is the set rather than the reset state.
    ///
    /// Case: a terminal is spawned and the shell echoes a command line
    /// longer than the window is wide.
    #[test]
    fn autowrap_starts_enabled() {
        assert_eq!(VtModes::default().auto_wrap, AutoWrap::Enabled);
    }

    /// Asserts that `DECSET 7` selects `Enabled` and `DECRST 7` selects
    /// `Disabled`, and that `wraps` reports the selection.
    ///
    /// Case: a status-bar program turns autowrap off to draw a
    /// full-width label and turns it back on before handing the
    /// terminal back to the shell.
    #[test]
    fn decset_seven_selects_enabled_and_decrst_selects_disabled() {
        assert_eq!(AutoWrap::from_decset(true), AutoWrap::Enabled);
        assert_eq!(AutoWrap::from_decset(false), AutoWrap::Disabled);
        assert!(AutoWrap::Enabled.wraps());
        assert!(!AutoWrap::Disabled.wraps());
    }

    /// Asserts that a device that has seen no DECTCEM starts with the
    /// cursor shown, which is the mode's documented default.
    ///
    /// Case: a terminal is spawned and the shell prints its first
    /// prompt before any application has touched cursor visibility.
    #[test]
    fn the_text_cursor_starts_shown() {
        assert_eq!(
            VtModes::default().text_cursor.enable,
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
