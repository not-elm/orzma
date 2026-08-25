//! The DECSET / DECRST modes the device carries and the enums they
//! select among.

/// Snapshot of the input-relevant terminal modes.
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

/// Which of a device's two screens is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScreenKind {
    /// The scrollback-backed screen a shell writes to.
    #[default]
    Primary,
    /// The scrollback-free screen full-screen applications take over.
    Alternate,
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
