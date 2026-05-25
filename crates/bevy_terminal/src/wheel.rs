//! Pure mouse-wheel routing for the Bevy terminal.
//!
//! This module is Bevy- and PTY-agnostic. The Bevy `mouse_wheel_system`
//! collects raw input, accumulates fractional Pixel deltas, resolves the
//! cursor cell, and calls `route_wheel` to decide what bytes (if any)
//! to send to the PTY and whether to scroll the host viewport.
//!
//! Three routing paths, in priority order:
//!
//! 1. **Mouse protocol** — when any of `MOUSE_REPORT_CLICK`, `MOUSE_DRAG`,
//!    `MOUSE_MOTION` is set, emit one SGR (or X10 fallback) wheel report
//!    per notch (`min(|notches|, max_protocol_events_per_frame)`).
//! 2. **Alt-screen translation** — when `ALT_SCREEN | ALTERNATE_SCROLL`
//!    is set and Shift is not held, translate to `Esc O A` / `Esc O B`
//!    (always SS3, regardless of `APP_CURSOR`) repeated `lines` times.
//! 3. **Scrollback** — otherwise, scroll the host viewport by `lines`.
//!
//! `lines = notches * (mods.fine ? fine_lines : lines_per_notch)`.
//! The mouse-protocol path intentionally does NOT multiply by
//! `lines_per_notch`; the application decides how many lines a notch
//! means (this matches alacritty: in mouse mode the multiplier is
//! forced to 1).

use alacritty_terminal::term::TermMode;

/// Configuration for wheel routing. Mirrors the `[mouse]` config block.
#[derive(Clone, Debug)]
pub struct WheelConfig {
    /// Lines scrolled per notch in the scrollback / alt-screen paths.
    pub lines_per_notch: u32,
    /// Lines scrolled per notch when `mods.fine` is set.
    pub fine_lines: u32,
    /// Upper bound on SGR/X10 events emitted from a single
    /// `route_wheel` call. Protects the PTY from input bursts.
    pub max_protocol_events_per_frame: u32,
}

impl Default for WheelConfig {
    fn default() -> Self {
        Self {
            lines_per_notch: 3,
            fine_lines: 1,
            max_protocol_events_per_frame: 8,
        }
    }
}

/// 1-indexed cell coordinate suitable for SGR / X10 mouse reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellCoord {
    pub col: u32,
    pub row: u32,
}

/// Wheel direction. Horizontal wheels are out of scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WheelDir {
    Up,
    Down,
}

/// Modifier state captured at the moment the wheel event fires.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WheelModifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    /// Resolved by the Bevy system layer from `cfg.fine_modifier`
    /// before calling `route_wheel`.
    pub fine: bool,
}

/// What `route_wheel` decided.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WheelAction {
    /// Scroll the host viewport by this many lines. Positive = down
    /// (toward live tail, decreases `display_offset`); negative = up
    /// (older lines, increases `display_offset`).
    ScrollViewport(i32),
    /// Send these bytes to the PTY (pre-encoded, possibly multiple
    /// reports concatenated).
    WriteToPty(Vec<u8>),
    /// Nothing to do this frame.
    Noop,
}

/// Encodes a single SGR (1006) wheel report.
///
/// Wire format: `CSI < <b> ; <col> ; <row> M` (press only; wheel never
/// reports release). `<b>` is `64` for up, `65` for down, plus `+4`
/// for Shift, `+8` for Alt, `+16` for Ctrl.
pub(crate) fn encode_sgr_wheel(
    direction: WheelDir,
    mods: WheelModifiers,
    cell: CellCoord,
) -> Vec<u8> {
    let mut b: u32 = 64;
    if matches!(direction, WheelDir::Down) {
        b += 1;
    }
    if mods.shift {
        b += 4;
    }
    if mods.alt {
        b += 8;
    }
    if mods.ctrl {
        b += 16;
    }
    format!("\x1b[<{};{};{}M", b, cell.col.max(1), cell.row.max(1)).into_bytes()
}

/// Encodes a single legacy X10 wheel report.
///
/// Wire format: `CSI M <b+32> <col+32> <row+32>`. Each coordinate
/// byte caps at 223 (`255 - 32`) — the X10 protocol cannot represent
/// larger cells; wheel reports for huge terminals must use SGR
/// (`SGR_MOUSE`).
pub(crate) fn encode_x10_wheel(
    direction: WheelDir,
    mods: WheelModifiers,
    cell: CellCoord,
) -> Vec<u8> {
    let mut b: u32 = 64;
    if matches!(direction, WheelDir::Down) {
        b += 1;
    }
    if mods.shift {
        b += 4;
    }
    if mods.alt {
        b += 8;
    }
    if mods.ctrl {
        b += 16;
    }
    let col_clamped = cell.col.clamp(1, 223) as u8;
    let row_clamped = cell.row.clamp(1, 223) as u8;
    vec![0x1b, b'[', b'M', (b + 32) as u8, col_clamped + 32, row_clamped + 32]
}

#[cfg(test)]
mod sgr_tests {
    use super::*;

    #[test]
    fn sgr_up_no_mods_origin() {
        let bytes = encode_sgr_wheel(WheelDir::Up, WheelModifiers::default(), CellCoord { col: 1, row: 1 });
        assert_eq!(bytes, b"\x1b[<64;1;1M");
    }

    #[test]
    fn sgr_down_no_mods_at_cell() {
        let bytes = encode_sgr_wheel(WheelDir::Down, WheelModifiers::default(), CellCoord { col: 43, row: 11 });
        assert_eq!(bytes, b"\x1b[<65;43;11M");
    }

    #[test]
    fn sgr_up_shift() {
        let mods = WheelModifiers { shift: true, ..Default::default() };
        let bytes = encode_sgr_wheel(WheelDir::Up, mods, CellCoord { col: 1, row: 1 });
        assert_eq!(bytes, b"\x1b[<68;1;1M");
    }

    #[test]
    fn sgr_up_ctrl_alt() {
        let mods = WheelModifiers { ctrl: true, alt: true, ..Default::default() };
        let bytes = encode_sgr_wheel(WheelDir::Up, mods, CellCoord { col: 1, row: 1 });
        // 64 + 8 (alt) + 16 (ctrl) = 88
        assert_eq!(bytes, b"\x1b[<88;1;1M");
    }

    #[test]
    fn sgr_zero_col_row_clamps_to_one() {
        let bytes = encode_sgr_wheel(WheelDir::Up, WheelModifiers::default(), CellCoord { col: 0, row: 0 });
        assert_eq!(bytes, b"\x1b[<64;1;1M");
    }
}

#[cfg(test)]
mod x10_tests {
    use super::*;

    #[test]
    fn x10_up_origin() {
        let bytes = encode_x10_wheel(WheelDir::Up, WheelModifiers::default(), CellCoord { col: 1, row: 1 });
        // ESC [ M  b(64+32=96)  x(1+32=33)  y(1+32=33)
        assert_eq!(bytes, vec![0x1b, b'[', b'M', 96, 33, 33]);
    }

    #[test]
    fn x10_down_cell() {
        let bytes = encode_x10_wheel(WheelDir::Down, WheelModifiers::default(), CellCoord { col: 10, row: 5 });
        // b = 65 + 32 = 97
        assert_eq!(bytes, vec![0x1b, b'[', b'M', 97, 42, 37]);
    }

    #[test]
    fn x10_clamps_beyond_223() {
        let bytes = encode_x10_wheel(WheelDir::Up, WheelModifiers::default(), CellCoord { col: 300, row: 300 });
        // 223 + 32 = 255
        assert_eq!(bytes, vec![0x1b, b'[', b'M', 96, 255, 255]);
    }

    #[test]
    fn x10_with_shift_ctrl() {
        let mods = WheelModifiers { shift: true, ctrl: true, ..Default::default() };
        let bytes = encode_x10_wheel(WheelDir::Down, mods, CellCoord { col: 1, row: 1 });
        // 65 + 4 + 16 = 85, + 32 = 117
        assert_eq!(bytes, vec![0x1b, b'[', b'M', 117, 33, 33]);
    }
}
