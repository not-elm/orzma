//! Pure VT-encoder for mouse-protocol reports. Translates a logical
//! mouse report into the byte sequence the PTY expects. No I/O, no
//! Bevy types — kept pure so unit tests can cover every branch.

use orzma_vt::prelude::MouseEncoding;

/// 1-indexed cell coordinate suitable for SGR / X10 mouse reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellCoord {
    pub col: u32,
    pub row: u32,
}
/// Wheel direction (vertical and horizontal).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WheelDir {
    Up,
    Down,
    Left,
    Right,
}

/// Mouse-protocol modifier set, mapped onto the report's `cb` bits
/// (shift=4, alt/meta=8, ctrl=16).
///
/// OS-level Alt (Option on macOS) is xterm's "meta" bit: `alt` and
/// `meta` merge into the single +8 bit and never double-count.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProtocolModifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}

/// Button identity carried by a mouse report.
///
/// Wheel variants are press-shaped: xterm reports them with button
/// codes 64..=67 and never emits a release or sets the motion bit
/// for them, so wheel reports use [`MouseReportKind::Press`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
    WheelLeft,
    WheelRight,
    //TODO: [Mouse Button 8-11](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html#h3-Other-buttons)までの対応をする必要があれば対応。
}

impl MouseButton {
    fn cb_base(self) -> u32 {
        match self {
            Self::Left => 0,
            Self::Middle => 1,
            Self::Right => 2,
            Self::WheelUp => 64,
            Self::WheelDown => 65,
            Self::WheelLeft => 66,
            Self::WheelRight => 67,
        }
    }
}

/// What produced the report. `Drag` is "motion while the button is
/// held" and is the only kind that sets the +32 motion bit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseReportKind {
    Press,
    Drag,
    Release,
}

/// One mouse-protocol report bound for the PTY.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MouseReport {
    pub button: MouseButton,
    pub kind: MouseReportKind,
    pub cell: CellCoord,
    pub mods: ProtocolModifiers,
}

impl MouseReport {
    /// Encodes this report in the given mouse encoding. UTF-8 (1005)
    /// is not implemented and intentionally falls back to X10 framing
    /// (byte-identical for coordinates <= 95).
    pub fn encode(&self, encoding: MouseEncoding) -> Vec<u8> {
        match encoding {
            MouseEncoding::Sgr => self.encode_sgr(),
            // TODO: real 1005 support (UTF-8-encode cb/col/row above 95, cap 2015).
            MouseEncoding::Utf8 | MouseEncoding::X10 => self.encode_x10(),
        }
    }

    /// `ESC [ < cb ; col ; row {M|m}` — release keeps the button code
    /// and switches the final byte to lowercase `m`.
    fn encode_sgr(&self) -> Vec<u8> {
        let cb = self.cb_bits(self.button.cb_base());
        let final_byte = if matches!(self.kind, MouseReportKind::Release) {
            'm'
        } else {
            'M'
        };
        format!(
            "\x1b[<{};{};{}{}",
            cb,
            self.cell.col.max(1),
            self.cell.row.max(1),
            final_byte
        )
        .into_bytes()
    }

    /// `ESC [ M <cb+32> <col+32> <row+32>` with coords clamped to
    /// `1..=223`. Release replaces the button base with the all-released
    /// sentinel 3; modifier and motion bits still apply on top, so e.g.
    /// Shift-release keeps its Shift bit.
    fn encode_x10(&self) -> Vec<u8> {
        let base = if matches!(self.kind, MouseReportKind::Release) {
            3
        } else {
            self.button.cb_base()
        };
        let cb = self.cb_bits(base);
        let col = self.cell.col.clamp(1, 223) as u8;
        let row = self.cell.row.clamp(1, 223) as u8;
        vec![0x1b, b'[', b'M', (cb + 32) as u8, col + 32, row + 32]
    }

    fn cb_bits(&self, base: u32) -> u32 {
        let mut cb = base;
        if matches!(self.kind, MouseReportKind::Drag) {
            cb += 32;
        }
        if self.mods.shift {
            cb += 4;
        }
        if self.mods.alt || self.mods.meta {
            cb += 8;
        }
        if self.mods.ctrl {
            cb += 16;
        }
        cb
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(button: MouseButton, kind: MouseReportKind, col: u32, row: u32) -> MouseReport {
        MouseReport {
            button,
            kind,
            cell: CellCoord { col, row },
            mods: ProtocolModifiers::default(),
        }
    }

    #[test]
    fn sgr_left_press_no_mods() {
        let r = report(MouseButton::Left, MouseReportKind::Press, 5, 7);
        assert_eq!(r.encode(MouseEncoding::Sgr), b"\x1b[<0;5;7M");
    }

    #[test]
    fn sgr_left_drag_sets_motion_bit() {
        let r = report(MouseButton::Left, MouseReportKind::Drag, 1, 1);
        assert_eq!(r.encode(MouseEncoding::Sgr), b"\x1b[<32;1;1M");
    }

    #[test]
    fn sgr_release_uses_lowercase_m() {
        let r = report(MouseButton::Left, MouseReportKind::Release, 2, 3);
        assert_eq!(r.encode(MouseEncoding::Sgr), b"\x1b[<0;2;3m");
    }

    #[test]
    fn sgr_middle_and_right_button_codes() {
        let middle = report(MouseButton::Middle, MouseReportKind::Press, 1, 1);
        let right = report(MouseButton::Right, MouseReportKind::Press, 1, 1);
        assert_eq!(middle.encode(MouseEncoding::Sgr), b"\x1b[<1;1;1M");
        assert_eq!(right.encode(MouseEncoding::Sgr), b"\x1b[<2;1;1M");
    }

    #[test]
    fn sgr_wheel_up_with_shift_and_ctrl() {
        let mut r = report(MouseButton::WheelUp, MouseReportKind::Press, 10, 20);
        r.mods.shift = true;
        r.mods.ctrl = true;
        // 64 + 4 (shift) + 16 (ctrl) = 84 — wheel does NOT add the motion bit.
        assert_eq!(r.encode(MouseEncoding::Sgr), b"\x1b[<84;10;20M");
    }

    #[test]
    fn sgr_wheel_direction_codes() {
        let down = report(MouseButton::WheelDown, MouseReportKind::Press, 1, 1);
        let left = report(MouseButton::WheelLeft, MouseReportKind::Press, 1, 1);
        let right = report(MouseButton::WheelRight, MouseReportKind::Press, 1, 1);
        assert_eq!(down.encode(MouseEncoding::Sgr), b"\x1b[<65;1;1M");
        assert_eq!(left.encode(MouseEncoding::Sgr), b"\x1b[<66;1;1M");
        assert_eq!(right.encode(MouseEncoding::Sgr), b"\x1b[<67;1;1M");
    }

    #[test]
    fn sgr_coords_floor_at_1() {
        let r = report(MouseButton::Left, MouseReportKind::Press, 0, 0);
        assert_eq!(r.encode(MouseEncoding::Sgr), b"\x1b[<0;1;1M");
    }

    #[test]
    fn alt_modifier_sets_meta_bit_in_sgr() {
        let mut r = report(MouseButton::Left, MouseReportKind::Press, 5, 5);
        r.mods.alt = true;
        assert_eq!(r.encode(MouseEncoding::Sgr), b"\x1b[<8;5;5M");
    }

    #[test]
    fn alt_and_meta_dont_double_count() {
        let mut r = report(MouseButton::Left, MouseReportKind::Press, 5, 5);
        r.mods.alt = true;
        r.mods.meta = true;
        assert_eq!(r.encode(MouseEncoding::Sgr), b"\x1b[<8;5;5M");
    }

    #[test]
    fn x10_left_press_offset_32() {
        let r = report(MouseButton::Left, MouseReportKind::Press, 1, 1);
        assert_eq!(
            r.encode(MouseEncoding::X10),
            vec![0x1b, b'[', b'M', 32, 33, 33]
        );
    }

    #[test]
    fn x10_release_uses_cb_base_3() {
        let r = report(MouseButton::Left, MouseReportKind::Release, 1, 1);
        // cb_base = 3 (release sentinel) + 32 = 35
        assert_eq!(
            r.encode(MouseEncoding::X10),
            vec![0x1b, b'[', b'M', 35, 33, 33]
        );
    }

    #[test]
    fn x10_release_keeps_modifier_bits() {
        let mut r = report(MouseButton::Left, MouseReportKind::Release, 1, 1);
        r.mods.shift = true;
        // 3 (release sentinel) + 4 (shift) + 32 = 39
        assert_eq!(
            r.encode(MouseEncoding::X10),
            vec![0x1b, b'[', b'M', 39, 33, 33]
        );
    }

    #[test]
    fn x10_coords_clamp_at_223() {
        let r = report(MouseButton::Left, MouseReportKind::Press, 500, 9999);
        assert_eq!(
            r.encode(MouseEncoding::X10),
            vec![0x1b, b'[', b'M', 32, 255, 255]
        );
    }

    // Documents the current policy: 1005 is unimplemented and encodes
    // exactly like X10.
    #[test]
    fn utf8_currently_falls_back_to_x10() {
        let r = report(MouseButton::Left, MouseReportKind::Press, 150, 42);
        assert_eq!(r.encode(MouseEncoding::Utf8), r.encode(MouseEncoding::X10));
    }
}
