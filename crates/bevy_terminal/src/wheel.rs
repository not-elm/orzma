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

pub use crate::mouse_encode::CellCoord;

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

fn protocol_mods_from(mods: WheelModifiers) -> crate::mouse_encode::ProtocolModifiers {
    crate::mouse_encode::ProtocolModifiers {
        shift: mods.shift,
        ctrl: mods.ctrl,
        // NOTE: WheelModifiers.alt → ProtocolModifiers.meta. The xterm
        // "meta" bit (+8) is the Alt/Option key on macOS and Linux; the
        // legacy `encode_sgr_wheel` mapped `mods.alt` to that bit
        // directly. Mapping alt → ProtocolModifiers.alt has no
        // SGR-byte assignment and would drop the bit entirely.
        alt: false,
        meta: mods.alt,
    }
}

/// Encodes a single wheel report (SGR or X10, picked by `modes`).
///
/// Wire format follows the shared protocol encoder. `<cb>` is `64` for
/// up, `65` for down, plus `+4` for Shift, `+8` for Alt (Alt/Option →
/// xterm meta bit), and `+16` for Ctrl. Alacritty's wheel-report
/// convention does NOT set the motion bit (+32) on wheel events, so
/// `motion = false` is passed through.
fn encode_wheel_report(
    modes: TermMode,
    direction: WheelDir,
    mods: WheelModifiers,
    cell: CellCoord,
) -> Vec<u8> {
    let cb_base: u8 = match direction {
        WheelDir::Up => 64,
        WheelDir::Down => 65,
    };
    crate::mouse_encode::encode_protocol_event(
        modes,
        cb_base,
        cell,
        protocol_mods_from(mods),
        false,
        false,
    )
}

/// Decides what to do with a discrete wheel input.
///
/// `notches` is sign-significant (negative = up / older). The router
/// dispatches in the priority order documented at the module top:
///
/// 1. Mouse protocol — when any of `MOUSE_REPORT_CLICK`, `MOUSE_DRAG`,
///    `MOUSE_MOTION` is set. Emits `min(|notches|, max_protocol_events_per_frame)`
///    reports. Uses SGR when `SGR_MOUSE` is set, falls back to X10 otherwise.
/// 2. Alt-screen — when `ALT_SCREEN | ALTERNATE_SCROLL` is set and
///    Shift is not held. Emits `|notches * lines_per_notch|` SS3
///    arrow sequences.
/// 3. Scrollback — otherwise. Returns `ScrollViewport(+lines)` for
///    upward notches (offset grows toward history).
///
/// The `mouse_cell` argument is only consulted for the mouse-protocol
/// path; pass `CellCoord { col: 1, row: 1 }` when unknown.
pub fn route_wheel(
    modes: TermMode,
    notches: i32,
    mouse_cell: CellCoord,
    mods: WheelModifiers,
    cfg: &WheelConfig,
) -> WheelAction {
    if notches == 0 {
        return WheelAction::Noop;
    }
    let direction = if notches < 0 {
        WheelDir::Up
    } else {
        WheelDir::Down
    };

    let any_mouse = modes
        .intersects(TermMode::MOUSE_REPORT_CLICK | TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION);
    if any_mouse {
        let count = (notches.unsigned_abs()).min(cfg.max_protocol_events_per_frame);
        if count == 0 {
            return WheelAction::Noop;
        }
        let mut buf = Vec::new();
        for _ in 0..count {
            let one = encode_wheel_report(modes, direction, mods, mouse_cell);
            buf.extend_from_slice(&one);
        }
        return WheelAction::WriteToPty(buf);
    }

    // NOTE: no Shift bypass to host scrollback here. `scroll_display`
    // would act on the active (alt) buffer, which alacritty_terminal
    // keeps without scrollback history, so the gesture would silently
    // no-op. wezterm / foot / kitty all route alt-screen wheel
    // straight to arrow keys; we match that convention. To view host
    // scrollback while inside an alt-screen app, use copy mode or
    // exit the app.
    if modes.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
        let lines_per = if mods.fine {
            cfg.fine_lines
        } else {
            cfg.lines_per_notch
        };
        let n = notches.unsigned_abs().saturating_mul(lines_per);
        if n == 0 {
            return WheelAction::Noop;
        }
        return WheelAction::WriteToPty(alt_screen_arrow_bytes(direction, n));
    }

    let lines_per = if mods.fine {
        cfg.fine_lines
    } else {
        cfg.lines_per_notch
    } as i32;
    let viewport_delta = -notches * lines_per;
    WheelAction::ScrollViewport(viewport_delta)
}

/// Emits `n` SS3-form arrow-key sequences for the alt-screen
/// translation path. `Esc O A` (Up) and `Esc O B` (Down) are sent
/// regardless of `APP_CURSOR` (DECCKM); this matches the convention
/// in alacritty and wezterm — DECCKM affects keyboard-originated
/// cursor keys, but wheel→arrow translation in alt-screen mode is
/// unconditional SS3.
pub(crate) fn alt_screen_arrow_bytes(direction: WheelDir, n: u32) -> Vec<u8> {
    let suffix = match direction {
        WheelDir::Up => b'A',
        WheelDir::Down => b'B',
    };
    let mut out = Vec::with_capacity(n as usize * 3);
    for _ in 0..n {
        out.extend_from_slice(&[0x1b, b'O', suffix]);
    }
    out
}

#[cfg(test)]
mod sgr_tests {
    use super::*;

    fn sgr(direction: WheelDir, mods: WheelModifiers, cell: CellCoord) -> Vec<u8> {
        encode_wheel_report(TermMode::SGR_MOUSE, direction, mods, cell)
    }

    #[test]
    fn sgr_up_no_mods_origin() {
        let bytes = sgr(
            WheelDir::Up,
            WheelModifiers::default(),
            CellCoord { col: 1, row: 1 },
        );
        assert_eq!(bytes, b"\x1b[<64;1;1M");
    }

    #[test]
    fn sgr_down_no_mods_at_cell() {
        let bytes = sgr(
            WheelDir::Down,
            WheelModifiers::default(),
            CellCoord { col: 43, row: 11 },
        );
        assert_eq!(bytes, b"\x1b[<65;43;11M");
    }

    #[test]
    fn sgr_up_shift() {
        let mods = WheelModifiers {
            shift: true,
            ..Default::default()
        };
        let bytes = sgr(WheelDir::Up, mods, CellCoord { col: 1, row: 1 });
        assert_eq!(bytes, b"\x1b[<68;1;1M");
    }

    #[test]
    fn sgr_up_ctrl_alt() {
        let mods = WheelModifiers {
            ctrl: true,
            alt: true,
            ..Default::default()
        };
        let bytes = sgr(WheelDir::Up, mods, CellCoord { col: 1, row: 1 });
        // 64 + 8 (alt) + 16 (ctrl) = 88
        assert_eq!(bytes, b"\x1b[<88;1;1M");
    }

    #[test]
    fn sgr_zero_col_row_clamps_to_one() {
        let bytes = sgr(
            WheelDir::Up,
            WheelModifiers::default(),
            CellCoord { col: 0, row: 0 },
        );
        assert_eq!(bytes, b"\x1b[<64;1;1M");
    }
}

#[cfg(test)]
mod x10_tests {
    use super::*;

    fn x10(direction: WheelDir, mods: WheelModifiers, cell: CellCoord) -> Vec<u8> {
        // Any non-SGR mouse mode bit drops into the X10 branch.
        encode_wheel_report(TermMode::MOUSE_REPORT_CLICK, direction, mods, cell)
    }

    #[test]
    fn x10_up_origin() {
        let bytes = x10(
            WheelDir::Up,
            WheelModifiers::default(),
            CellCoord { col: 1, row: 1 },
        );
        // ESC [ M  b(64+32=96)  x(1+32=33)  y(1+32=33)
        assert_eq!(bytes, vec![0x1b, b'[', b'M', 96, 33, 33]);
    }

    #[test]
    fn x10_down_cell() {
        let bytes = x10(
            WheelDir::Down,
            WheelModifiers::default(),
            CellCoord { col: 10, row: 5 },
        );
        // b = 65 + 32 = 97
        assert_eq!(bytes, vec![0x1b, b'[', b'M', 97, 42, 37]);
    }

    #[test]
    fn x10_clamps_beyond_223() {
        let bytes = x10(
            WheelDir::Up,
            WheelModifiers::default(),
            CellCoord { col: 300, row: 300 },
        );
        // 223 + 32 = 255
        assert_eq!(bytes, vec![0x1b, b'[', b'M', 96, 255, 255]);
    }

    #[test]
    fn x10_with_shift_ctrl() {
        let mods = WheelModifiers {
            shift: true,
            ctrl: true,
            ..Default::default()
        };
        let bytes = x10(WheelDir::Down, mods, CellCoord { col: 1, row: 1 });
        // 65 + 4 + 16 = 85, + 32 = 117
        assert_eq!(bytes, vec![0x1b, b'[', b'M', 117, 33, 33]);
    }
}

#[cfg(test)]
mod alt_screen_tests {
    use super::*;

    #[test]
    fn alt_screen_up_three() {
        let bytes = alt_screen_arrow_bytes(WheelDir::Up, 3);
        assert_eq!(bytes, b"\x1bOA\x1bOA\x1bOA");
    }

    #[test]
    fn alt_screen_down_one() {
        let bytes = alt_screen_arrow_bytes(WheelDir::Down, 1);
        assert_eq!(bytes, b"\x1bOB");
    }

    #[test]
    fn alt_screen_zero_returns_empty() {
        let bytes = alt_screen_arrow_bytes(WheelDir::Up, 0);
        assert!(bytes.is_empty());
    }
}

#[cfg(test)]
mod route_tests {
    use super::*;
    use alacritty_terminal::term::TermMode;

    fn cfg_default() -> WheelConfig {
        WheelConfig::default()
    }

    fn cell() -> CellCoord {
        CellCoord { col: 1, row: 1 }
    }

    #[test]
    fn noop_on_zero_notches() {
        let action = route_wheel(
            TermMode::empty(),
            0,
            cell(),
            WheelModifiers::default(),
            &cfg_default(),
        );
        assert_eq!(action, WheelAction::Noop);
    }

    #[test]
    fn scrollback_when_no_special_mode() {
        // notches=-1 means scroll up (older); lines_per_notch=3 → scroll viewport by +3
        let action = route_wheel(
            TermMode::empty(),
            -1,
            cell(),
            WheelModifiers::default(),
            &cfg_default(),
        );
        assert_eq!(action, WheelAction::ScrollViewport(3));
    }

    #[test]
    fn scrollback_fine_modifier_uses_fine_lines() {
        let mods = WheelModifiers {
            fine: true,
            ..Default::default()
        };
        let action = route_wheel(TermMode::empty(), -2, cell(), mods, &cfg_default());
        // fine_lines = 1, so -2 notches * 1 line/notch = -2 → ScrollViewport(+2)
        assert_eq!(action, WheelAction::ScrollViewport(2));
    }

    #[test]
    fn alt_screen_translates_to_arrows() {
        let modes = TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL;
        let action = route_wheel(modes, -1, cell(), WheelModifiers::default(), &cfg_default());
        // -1 notch * 3 lines = 3 up arrows
        assert_eq!(
            action,
            WheelAction::WriteToPty(b"\x1bOA\x1bOA\x1bOA".to_vec())
        );
    }

    #[test]
    fn alt_screen_translates_down_arrows() {
        let modes = TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL;
        let action = route_wheel(modes, 1, cell(), WheelModifiers::default(), &cfg_default());
        // +1 notch * 3 lines = 3 down arrows
        assert_eq!(
            action,
            WheelAction::WriteToPty(b"\x1bOB\x1bOB\x1bOB".to_vec())
        );
    }

    #[test]
    fn alt_screen_without_alternate_scroll_falls_back_to_scrollback() {
        // App disabled ?1007 — scrollback wins
        let modes = TermMode::ALT_SCREEN;
        let action = route_wheel(modes, -1, cell(), WheelModifiers::default(), &cfg_default());
        assert_eq!(action, WheelAction::ScrollViewport(3));
    }

    #[test]
    fn alt_screen_shift_alone_still_translates_to_arrows() {
        // No Shift-bypass: pressing Shift without the fine modifier set
        // does not change routing — alt-screen still receives arrow keys
        // at the normal lines_per_notch rate.
        let modes = TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL;
        let mods = WheelModifiers {
            shift: true,
            ..Default::default()
        };
        let action = route_wheel(modes, -1, cell(), mods, &cfg_default());
        assert_eq!(
            action,
            WheelAction::WriteToPty(b"\x1bOA\x1bOA\x1bOA".to_vec())
        );
    }

    #[test]
    fn alt_screen_fine_modifier_sends_fewer_arrows() {
        // Shift+wheel under default config sets `mods.fine = true` so the
        // alt-screen path emits `fine_lines` arrows per notch instead of
        // `lines_per_notch`. Matches wezterm's `alternate_buffer_wheel_scroll_speed`
        // policy and lets users slow-scroll inside vim/less.
        let modes = TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL;
        let mods = WheelModifiers {
            shift: true,
            fine: true,
            ..Default::default()
        };
        let action = route_wheel(modes, -1, cell(), mods, &cfg_default());
        // -1 notch * fine_lines=1 = 1 up arrow
        assert_eq!(action, WheelAction::WriteToPty(b"\x1bOA".to_vec()));
    }

    #[test]
    fn sgr_mouse_mode_emits_one_sgr_event() {
        let modes = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        let action = route_wheel(
            modes,
            -1,
            CellCoord { col: 43, row: 11 },
            WheelModifiers::default(),
            &cfg_default(),
        );
        assert_eq!(action, WheelAction::WriteToPty(b"\x1b[<64;43;11M".to_vec()));
    }

    #[test]
    fn x10_mouse_mode_emits_one_x10_event() {
        let modes = TermMode::MOUSE_DRAG; // no SGR_MOUSE
        let action = route_wheel(
            modes,
            1,
            CellCoord { col: 1, row: 1 },
            WheelModifiers::default(),
            &cfg_default(),
        );
        assert_eq!(
            action,
            WheelAction::WriteToPty(vec![0x1b, b'[', b'M', 97, 33, 33])
        );
    }

    #[test]
    fn sgr_mouse_concats_multiple_notches() {
        let modes = TermMode::MOUSE_DRAG | TermMode::SGR_MOUSE;
        let action = route_wheel(
            modes,
            3,
            CellCoord { col: 1, row: 1 },
            WheelModifiers::default(),
            &cfg_default(),
        );
        let one = b"\x1b[<65;1;1M";
        let mut expected = Vec::new();
        expected.extend_from_slice(one);
        expected.extend_from_slice(one);
        expected.extend_from_slice(one);
        assert_eq!(action, WheelAction::WriteToPty(expected));
    }

    #[test]
    fn sgr_mouse_clamps_to_max_events_per_frame() {
        let modes = TermMode::MOUSE_DRAG | TermMode::SGR_MOUSE;
        let cfg = WheelConfig {
            max_protocol_events_per_frame: 4,
            ..WheelConfig::default()
        };
        let action = route_wheel(
            modes,
            20,
            CellCoord { col: 1, row: 1 },
            WheelModifiers::default(),
            &cfg,
        );
        let one = b"\x1b[<65;1;1M";
        let mut expected = Vec::new();
        for _ in 0..4 {
            expected.extend_from_slice(one);
        }
        assert_eq!(action, WheelAction::WriteToPty(expected));
    }

    #[test]
    fn sgr_mouse_with_zero_cap_returns_noop() {
        let modes = TermMode::MOUSE_DRAG | TermMode::SGR_MOUSE;
        let cfg = WheelConfig {
            max_protocol_events_per_frame: 0,
            ..WheelConfig::default()
        };
        let action = route_wheel(modes, 5, cell(), WheelModifiers::default(), &cfg);
        assert_eq!(action, WheelAction::Noop);
    }

    #[test]
    fn mouse_protocol_takes_priority_over_alt_screen() {
        let modes = TermMode::MOUSE_DRAG
            | TermMode::SGR_MOUSE
            | TermMode::ALT_SCREEN
            | TermMode::ALTERNATE_SCROLL;
        let action = route_wheel(
            modes,
            1,
            CellCoord { col: 1, row: 1 },
            WheelModifiers::default(),
            &cfg_default(),
        );
        assert_eq!(action, WheelAction::WriteToPty(b"\x1b[<65;1;1M".to_vec()));
    }
}
