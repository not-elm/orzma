// /// Configuration for wheel routing. Mirrors the `[mouse]` config block.
// #[derive(Clone, Debug)]
// pub struct WheelConfig {
//     /// Lines scrolled per notch in the scrollback / alt-screen paths.
//     pub lines_per_notch: u32,
//     /// Lines scrolled per notch when `mods.fine` is set.
//     pub fine_lines: u32,
//     /// Upper bound on SGR/X10 events emitted from a single
//     /// `WheelAction::route` call. Protects the PTY from input bursts.
//     pub max_protocol_events_per_frame: u32,
// }
//
// impl Default for WheelConfig {
//     fn default() -> Self {
//         Self {
//             lines_per_notch: 3,
//             fine_lines: 1,
//             max_protocol_events_per_frame: 8,
//         }
//     }
// }
//
// /// What `WheelAction::route` decided.
// #[derive(Clone, Debug, PartialEq, Eq)]
// pub enum WheelAction {
//     /// Scroll the host viewport by this many lines. Positive = down
//     /// (toward live tail, decreases `display_offset`); negative = up
//     /// (older lines, increases `display_offset`).
//     ScrollViewport(i32),
//     /// Send these bytes to the PTY (pre-encoded, possibly multiple
//     /// reports concatenated).
//     WriteToPty(Vec<u8>),
//     /// Nothing to do this frame.
//     Noop,
// }
//
// /// Encodes a single wheel report (SGR or X10, picked by `modes`).
// ///
// /// Wire format follows the shared protocol encoder. `<cb>` is `64` for
// /// up, `65` for down, plus `+4` for Shift, `+8` for Alt (Alt/Option →
// /// xterm meta bit), and `+16` for Ctrl. Alacritty's wheel-report
// /// convention does NOT set the motion bit (+32) on wheel events, so
// /// `motion = false` is passed through.
// fn encode_wheel_report(
//     modes: TermMode,
//     direction: WheelDir,
//     mods: WheelModifiers,
//     cell: CellCoord,
// ) -> Vec<u8> {
//     let cb_base: u8 = match direction {
//         WheelDir::Up => 64,
//         WheelDir::Down => 65,
//         WheelDir::Left => 66,
//         WheelDir::Right => 67,
//     };
//     encode_protocol_event(modes, cb_base, cell, protocol_mods_from(mods), false, false)
// }
//
// /// Emits `min(|notches|, cap)` concatenated wheel reports for a mouse-mode
// /// pane, or `Noop` when the cap rounds the count to zero. Shared by the
// /// vertical (`route`) and horizontal (`route_horizontal`) mouse-protocol paths.
// fn emit_protocol_reports(
//     modes: TermMode,
//     direction: WheelDir,
//     notches: i32,
//     mouse_cell: CellCoord,
//     mods: WheelModifiers,
//     cfg: &WheelConfig,
// ) -> WheelAction {
//     let count = notches
//         .unsigned_abs()
//         .min(cfg.max_protocol_events_per_frame);
//     if count == 0 {
//         return WheelAction::Noop;
//     }
//     let mut buf = Vec::new();
//     for _ in 0..count {
//         buf.extend_from_slice(&encode_wheel_report(modes, direction, mods, mouse_cell));
//     }
//     WheelAction::WriteToPty(buf)
// }
//
// impl WheelAction {
//     /// Decides what to do with a discrete wheel input.
//     ///
//     /// `notches` is sign-significant (negative = up / older). The router
//     /// dispatches in the priority order documented at the module top:
//     ///
//     /// 1. Mouse protocol — when any of `MOUSE_REPORT_CLICK`, `MOUSE_DRAG`,
//     ///    `MOUSE_MOTION` is set. Emits `min(|notches|, max_protocol_events_per_frame)`
//     ///    reports. Uses SGR when `SGR_MOUSE` is set, falls back to X10 otherwise.
//     /// 2. Alt-screen — when `ALT_SCREEN | ALTERNATE_SCROLL` is set and
//     ///    Shift is not held. Emits `|notches * lines_per_notch|` SS3
//     ///    arrow sequences.
//     /// 3. Scrollback — otherwise. Returns `ScrollViewport(+lines)` for
//     ///    upward notches (offset grows toward history).
//     ///
//     /// The `mouse_cell` argument is only consulted for the mouse-protocol
//     /// path; pass `CellCoord { col: 1, row: 1 }` when unknown.
//     pub fn route(
//         modes: TermMode,
//         notches: i32,
//         mouse_cell: CellCoord,
//         mods: WheelModifiers,
//         cfg: &WheelConfig,
//     ) -> Self {
//         if notches == 0 {
//             return WheelAction::Noop;
//         }
//         let direction = if notches < 0 {
//             WheelDir::Up
//         } else {
//             WheelDir::Down
//         };
//
//         let any_mouse = modes.intersects(
//             TermMode::MOUSE_REPORT_CLICK | TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION,
//         );
//         if any_mouse {
//             return emit_protocol_reports(modes, direction, notches, mouse_cell, mods, cfg);
//         }
//
//         // NOTE: no Shift bypass to host scrollback here. `scroll_display`
//         // would act on the active (alt) buffer, which alacritty_terminal
//         // keeps without scrollback history, so the gesture would silently
//         // no-op. wezterm / foot / kitty all route alt-screen wheel
//         // straight to arrow keys; we match that convention. To view host
//         // scrollback while inside an alt-screen app, use vi mode or
//         // exit the app.
//         if modes.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
//             let lines_per = if mods.fine {
//                 cfg.fine_lines
//             } else {
//                 cfg.lines_per_notch
//             };
//             let n = notches.unsigned_abs().saturating_mul(lines_per);
//             if n == 0 {
//                 return WheelAction::Noop;
//             }
//             return WheelAction::WriteToPty(alt_screen_arrow_bytes(direction, n));
//         }
//
//         let lines_per = if mods.fine {
//             cfg.fine_lines
//         } else {
//             cfg.lines_per_notch
//         } as i32;
//         let viewport_delta = -notches * lines_per;
//         WheelAction::ScrollViewport(viewport_delta)
//     }
//
//     /// Decides what to do with a horizontal wheel input.
//     ///
//     /// `notches` is sign-significant (negative = left, positive = right).
//     /// Horizontal wheel only has meaning for mouse-mode applications: when any of
//     /// `MOUSE_REPORT_CLICK`, `MOUSE_DRAG`, `MOUSE_MOTION` is set, it emits
//     /// `min(|notches|, max_protocol_events_per_frame)` SGR/X10 reports with `cb`
//     /// 66 (left) / 67 (right). Outside a mouse mode there is no horizontal
//     /// scrollback or alt-screen translation, so it returns `Noop`.
//     pub fn route_horizontal(
//         modes: TermMode,
//         notches: i32,
//         mouse_cell: CellCoord,
//         mods: WheelModifiers,
//         cfg: &WheelConfig,
//     ) -> Self {
//         if notches == 0 {
//             return WheelAction::Noop;
//         }
//         let direction = if notches < 0 {
//             WheelDir::Left
//         } else {
//             WheelDir::Right
//         };
//         if modes.intersects(
//             TermMode::MOUSE_REPORT_CLICK | TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION,
//         ) {
//             return emit_protocol_reports(modes, direction, notches, mouse_cell, mods, cfg);
//         }
//         WheelAction::Noop
//     }
// }
//
// /// Emits `n` SS3-form arrow-key sequences for the alt-screen
// /// translation path. `Esc O A` (Up) and `Esc O B` (Down) are sent
// /// regardless of `APP_CURSOR` (DECCKM); this matches the convention
// /// in alacritty and wezterm — DECCKM affects keyboard-originated
// /// cursor keys, but wheel→arrow translation in alt-screen mode is
// /// unconditional SS3.
// fn alt_screen_arrow_bytes(direction: WheelDir, n: u32) -> Vec<u8> {
//     let suffix = match direction {
//         WheelDir::Up => b'A',
//         WheelDir::Down => b'B',
//         WheelDir::Left | WheelDir::Right => unreachable!(),
//     };
//     let mut out = Vec::with_capacity(n as usize * 3);
//     for _ in 0..n {
//         out.extend_from_slice(&[0x1b, b'O', suffix]);
//     }
//     out
// }
