//! Shared SGR / X10 mouse-protocol encoder. Consumed by both the wheel
//! router (`wheel.rs`) and the button router (`buttons.rs`). Pure
//! function — no I/O, no Bevy types.
//!
//! Encoding rules (spec §7):
//!
//! - `SGR_MOUSE` set → `ESC [ < <cb> ; <col> ; <row> {M|m}`. Release
//!   uses lowercase `m`. `<cb>` packs the button-base (0/1/2 for L/M/R
//!   press, 64+ for wheel) plus modifier bits (shift=4, meta=8,
//!   ctrl=16) plus motion bit (32) for drag/wheel reports.
//! - Otherwise (any other `MOUSE_MODE` bit set) → X10:
//!   `ESC [ M <cb+32> <col+32> <row+32>`. Coords clamp to `1..=223`.
//!   On release, X10 uses `cb_base = 3` (the "all released" sentinel)
//!   — modern apps that need release-button identity must use SGR.
//! - `UTF8_MOUSE` (1005) silently falls into the X10 branch. Apps that
//!   set 1005 without 1006 are extinct in 2026.

use alacritty_terminal::term::TermMode;

/// 1-indexed cell coordinate suitable for SGR / X10 mouse reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellCoord {
    pub col: u32,
    pub row: u32,
}

/// Shared mouse-protocol modifier set. `WheelModifiers` builds one of
/// these at the encoder call boundary; `ButtonAction::route` already
/// uses this type natively.
///
/// `WheelModifiers::fine` is NOT part of the protocol — it is router
/// policy that decides line counts — so it stays in `WheelModifiers`
/// and does not appear here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProtocolModifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}

/// Encodes one mouse-protocol report.
///
/// `cb_base` is the raw button code: 0/1/2 = L/M/R press, 64/65 = wheel
/// up/down. Caller is responsible for setting `motion = true` for drag
/// and for wheel reports (xterm treats wheel as a motion-bit press).
pub(crate) fn encode_protocol_event(
    modes: TermMode,
    cb_base: u8,
    cell: CellCoord,
    mods: ProtocolModifiers,
    motion: bool,
    release: bool,
) -> Vec<u8> {
    if modes.contains(TermMode::SGR_MOUSE) {
        encode_sgr(cb_base, cell, mods, motion, release)
    } else {
        encode_x10(cb_base, cell, mods, motion, release)
    }
}

fn encode_sgr(
    cb_base: u8,
    cell: CellCoord,
    mods: ProtocolModifiers,
    motion: bool,
    release: bool,
) -> Vec<u8> {
    let mut cb: u32 = cb_base as u32;
    if motion {
        cb += 32;
    }
    if mods.shift {
        cb += 4;
    }
    if mods.meta {
        cb += 8;
    }
    if mods.ctrl {
        cb += 16;
    }
    let final_byte = if release { 'm' } else { 'M' };
    format!(
        "\x1b[<{};{};{}{}",
        cb,
        cell.col.max(1),
        cell.row.max(1),
        final_byte
    )
    .into_bytes()
}

fn encode_x10(
    cb_base: u8,
    cell: CellCoord,
    mods: ProtocolModifiers,
    motion: bool,
    release: bool,
) -> Vec<u8> {
    // X10 release: emit cb_base = 3 (all-released sentinel). Modifier
    // bits + motion bit still encoded on top so e.g. Shift-release
    // still reports Shift.
    let mut cb: u32 = if release { 3 } else { cb_base as u32 };
    if motion {
        cb += 32;
    }
    if mods.shift {
        cb += 4;
    }
    if mods.meta {
        cb += 8;
    }
    if mods.ctrl {
        cb += 16;
    }
    let col = cell.col.clamp(1, 223) as u8;
    let row = cell.row.clamp(1, 223) as u8;
    vec![0x1b, b'[', b'M', (cb + 32) as u8, col + 32, row + 32]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(col: u32, row: u32) -> CellCoord {
        CellCoord { col, row }
    }

    #[test]
    fn sgr_left_press_no_mods() {
        let bytes = encode_protocol_event(
            TermMode::SGR_MOUSE,
            0,
            cell(5, 7),
            ProtocolModifiers::default(),
            false,
            false,
        );
        assert_eq!(bytes, b"\x1b[<0;5;7M");
    }

    #[test]
    fn sgr_left_drag_sets_motion_bit() {
        let bytes = encode_protocol_event(
            TermMode::SGR_MOUSE,
            0,
            cell(1, 1),
            ProtocolModifiers::default(),
            true,
            false,
        );
        assert_eq!(bytes, b"\x1b[<32;1;1M");
    }

    #[test]
    fn sgr_release_uses_lowercase_m() {
        let bytes = encode_protocol_event(
            TermMode::SGR_MOUSE,
            0,
            cell(2, 3),
            ProtocolModifiers::default(),
            false,
            true,
        );
        assert_eq!(bytes, b"\x1b[<0;2;3m");
    }

    #[test]
    fn sgr_wheel_up_with_shift_and_ctrl() {
        let bytes = encode_protocol_event(
            TermMode::SGR_MOUSE,
            64,
            cell(10, 20),
            ProtocolModifiers {
                shift: true,
                ctrl: true,
                ..Default::default()
            },
            false,
            false,
        );
        // 64 + 4 (shift) + 16 (ctrl) = 84
        assert_eq!(bytes, b"\x1b[<84;10;20M");
    }

    #[test]
    fn x10_left_press_offset_32() {
        let bytes = encode_protocol_event(
            TermMode::MOUSE_REPORT_CLICK,
            0,
            cell(1, 1),
            ProtocolModifiers::default(),
            false,
            false,
        );
        assert_eq!(bytes, vec![0x1b, b'[', b'M', 32, 33, 33]);
    }

    #[test]
    fn x10_release_uses_cb_base_3() {
        let bytes = encode_protocol_event(
            TermMode::MOUSE_REPORT_CLICK,
            0,
            cell(1, 1),
            ProtocolModifiers::default(),
            false,
            true,
        );
        // cb_base = 3 (release sentinel) + 32 = 35
        assert_eq!(bytes, vec![0x1b, b'[', b'M', 35, 33, 33]);
    }

    #[test]
    fn x10_coords_clamp_at_223() {
        let bytes = encode_protocol_event(
            TermMode::MOUSE_REPORT_CLICK,
            0,
            cell(500, 9999),
            ProtocolModifiers::default(),
            false,
            false,
        );
        assert_eq!(bytes, vec![0x1b, b'[', b'M', 32, 255, 255]);
    }

    #[test]
    fn utf8_mouse_falls_into_x10_path() {
        // UTF8_MOUSE without SGR_MOUSE → same bytes as X10.
        let bytes = encode_protocol_event(
            TermMode::UTF8_MOUSE,
            0,
            cell(1, 1),
            ProtocolModifiers::default(),
            false,
            false,
        );
        assert_eq!(bytes, vec![0x1b, b'[', b'M', 32, 33, 33]);
    }
}
