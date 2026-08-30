//! Byte-stream decoding: the vtparse parser and the synchronized-update
//! buffer.
//!
//! [`Interpreter`] turns PTY bytes into parser actions. It changes no
//! device state itself — the executor it dispatches to does that —
//! and it owns the CSI ?2026 buffer, so a synchronized update holds its
//! bytes here until the application closes it.

pub(crate) mod apc;

mod csi;
mod osc;
mod sgr;

use crate::device::modes::{KeypadMode, ScreenKind};
use crate::interpreter::csi::CsiParams;
use crate::interpreter::osc::window_title;
use crate::screen::character_sets::{CharacterSet, GCode, SingleShift};
use crate::screen::margins::OriginMode;
use crate::screen::tabs::CharacterTabEdit;
use crate::screen::{EraseLineMode, EraseScreenMode};
use crate::{
    InterpretOutput, VtSignal,
    device::DeviceState,
    frame::{FrameTracker, damage::DamageSpan},
};
use vtparse::{CsiParam, VTActor, VTParser};

/// The parser plus the bytes a synchronized update is holding back.
pub(crate) struct Interpreter {
    parser: VTParser,
    sync: SyncBuffer,
}

impl Interpreter {
    /// Decodes one chunk, applying each action to the borrowed
    /// components through an [`Executor`] and collecting everything the
    /// chunk produced into `output`.
    ///
    /// The executor is built here rather than passed in because it
    /// borrows [`SyncBuffer`], which `&mut self` already holds.
    pub fn parse(
        &mut self,
        output: &mut InterpretOutput,
        device: &mut DeviceState,
        tracker: &mut FrameTracker,
        chunk: &[u8],
    ) {
        let cursor_before = device.active_screen().cursor();
        let mut executor = Executor {
            output,
            sync: &mut self.sync,
            device,
            tracker,
        };
        self.parser.parse(chunk, &mut executor);
        executor.output.damaged |= cursor_before != executor.device.active_screen().cursor();
    }
}

impl Default for Interpreter {
    fn default() -> Self {
        Self {
            parser: VTParser::new(),
            sync: Default::default(),
        }
    }
}

/// Bytes held back while a synchronized update (CSI ?2026) is open.
#[derive(Default)]
struct SyncBuffer {}

/// The temporary view a parser callback applies its action through.
///
/// `output` borrows the caller's per-call local in
/// [`crate::OrzmaVt`]'s [`crate::Vt::interpret`].
/// The other fields borrow state that outlives the call — `device` and
/// `tracker` are components `OrzmaVt` owns, and `sync` reborrows
/// [`Interpreter::sync`], which persists across chunks. The view itself
/// still carries nothing between chunks: it is rebuilt fresh each call.
struct Executor<'a> {
    output: &'a mut InterpretOutput,
    sync: &'a mut SyncBuffer,
    device: &'a mut DeviceState,
    tracker: &'a mut FrameTracker,
}

impl VTActor for Executor<'_> {
    fn print(&mut self, b: char) {
        // NOTE: DEL is dropped before `Screen::print` rather than inside
        // it, so that it cannot spend a pending single shift — only a
        // graphic character may do that.
        if b == '\u{7f}' {
            return;
        }
        let damage = self.device.active_screen_mut().print(b);
        self.stage(damage);
    }

    fn execute_c0_or_c1(&mut self, control: u8) {
        match control {
            // BEL
            0x07 => self.signal(VtSignal::Bell),
            // BS
            0x08 => self.device.active_screen_mut().backspace(),
            // HT
            0x09 => self.device.active_screen_mut().move_forward_tabs(1),
            // LF, VT, FF, IND
            0x0A | 0x0B | 0x0C | 0x84 => self.index(),
            // CR
            0x0D => self.device.active_screen_mut().carriage_return(),
            // SO (LS1)
            0x0E => self.invoke_character_set(GCode::G1),
            // SI (LS0)
            0x0F => self.invoke_character_set(GCode::G0),
            // NEL
            0x85 => self.next_line(),
            // HTS
            0x88 => self.device.active_screen_mut().set_horizontal_tab_stop(),
            // RI
            0x8D => self.reverse_index(),
            // SS2
            0x8E => self.single_shift(SingleShift::G2),
            // SS3
            0x8F => self.single_shift(SingleShift::G3),
            // DECID
            0x9A => self.reply(PRIMARY_ATTRIBUTES),
            _ => {}
        }
    }

    // TODO: Implement the device control strings — Sixel (`DCS q`),
    // DECRQSS, and the user-defined keys — once the grid can carry
    // Sixel and DRCS glyphs and this crate can emit DCS replies for
    // DECRQSS. The three callbacks form one control function, so one
    // note covers all of them.
    fn dcs_hook(
        &mut self,
        _mode: u8,
        _params: &[i64],
        _intermediates: &[u8],
        _ignored_excess_intermediates: bool,
    ) {
    }

    fn dcs_put(&mut self, _byte: u8) {}

    fn dcs_unhook(&mut self) {}

    fn esc_dispatch(
        &mut self,
        _params: &[i64],
        intermediates: &[u8],
        _ignored_excess_intermediates: bool,
        byte: u8,
    ) {
        match (byte, intermediates) {
            // DECSC
            (b'7', []) => self.device.active_screen_mut().save_checkpoint(),
            // DECRC
            (b'8', []) => self.device.active_screen_mut().restore_checkpoint(),
            // DECKPAM
            (b'=', []) => self.device.modes_mut().keypad_mode = KeypadMode::Application,
            // DECKPNM
            (b'>', []) => self.device.modes_mut().keypad_mode = KeypadMode::Numeric,
            // IND
            (b'D', []) => self.index(),
            // NEL
            (b'E', []) => self.next_line(),
            // HTS
            (b'H', []) => self.device.active_screen_mut().set_horizontal_tab_stop(),
            // RI
            (b'M', []) => self.reverse_index(),
            // SS2
            (b'N', []) => self.single_shift(SingleShift::G2),
            // SS3
            (b'O', []) => self.single_shift(SingleShift::G3),
            // DECID
            (b'Z', []) => self.reply(PRIMARY_ATTRIBUTES),
            // ST
            (b'\\', []) => {}
            // RIS
            (b'c', []) => self.reset_device(),
            // LS2
            (b'n', []) => self.invoke_character_set(GCode::G2),
            // LS3
            (b'o', []) => self.invoke_character_set(GCode::G3),
            // DECALN
            (b'8', [b'#']) => {
                let damage = self.device.active_screen_mut().fill_alignment_pattern();
                self.stage(Some(damage));
            }
            // Select ISO 8859-1 (`ESC % @`) / UTF-8 (`ESC % G`)
            // NOTE: Ground is always decoded as UTF-8, so the ISO 8859-1
            // request is dropped rather than honored; honoring it needs a
            // byte-level decoding layer outside vtparse.
            (b'@' | b'G', [b'%']) => {}
            // SCS
            (dscs, [designator @ (b'(' | b')' | b'*' | b'+'), ..]) => {
                if let Some(g_code) = GCode::from_designator(*designator) {
                    self.device
                        .active_screen_mut()
                        .designate_character_set(g_code, CharacterSet::from_dscs(dscs));
                }
            }
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &[CsiParam], parameters_truncated: bool, byte: u8) {
        let params = CsiParams::parse(params);
        if parameters_truncated || params.has_intermediates() {
            return;
        }
        match (params.private(), byte) {
            // CUP, HVP
            (None, b'H' | b'f') => self
                .device
                .active_screen_mut()
                .move_cursor_to(params.value(0), params.value(1)),
            // DECSTBM
            (None, b'r') => self
                .device
                .active_screen_mut()
                .set_scroll_region(params.value(0), params.value(1)),
            // DA1
            (None, b'c') if params.value(0).unwrap_or(0) == 0 => self.reply(PRIMARY_ATTRIBUTES),
            // NOTE: Title reporting (`CSI 20 t` for the icon label, `CSI
            // 21 t` for the window title) is deliberately not implemented.
            // The real attack surface of terminal titles is the report
            // direction, not the set direction: an attacker sets a title
            // containing a shell command and then asks the terminal to
            // report it back into the shell's input. Several terminals
            // shipped exploitable versions of this (ConEmu's variant was
            // CVE-2022-46387 and CVE-2023-39150).
            // XTWINOPS 22
            (None, b't') if params.value(0) == Some(22) => self.device.push_title(),
            // XTWINOPS 23
            (None, b't') if params.value(0) == Some(23) => self.pop_title(),
            // CUU
            (None, b'A') => self
                .device
                .active_screen_mut()
                .move_cursor_up(repeat_count(params.value(0))),
            // CUD
            (None, b'B') => self
                .device
                .active_screen_mut()
                .move_cursor_down(repeat_count(params.value(0))),
            // CUF
            (None, b'C') => self
                .device
                .active_screen_mut()
                .move_cursor_right(repeat_count(params.value(0))),
            // CUB
            (None, b'D') => self
                .device
                .active_screen_mut()
                .move_cursor_left(repeat_count(params.value(0))),
            // CNL
            (None, b'E') => {
                let screen = self.device.active_screen_mut();
                screen.move_cursor_down(repeat_count(params.value(0)));
                screen.carriage_return();
            }
            // CPL
            (None, b'F') => {
                let screen = self.device.active_screen_mut();
                screen.move_cursor_up(repeat_count(params.value(0)));
                screen.carriage_return();
            }
            // ED
            (None, b'J') => {
                if let Some(mode) = EraseScreenMode::from_ed(params.value(0).unwrap_or(0)) {
                    let damage = self.device.active_screen_mut().erase_in_display(mode);
                    self.stage(damage);
                }
            }
            // EL
            (None, b'K') => {
                if let Some(mode) = EraseLineMode::from_el(params.value(0).unwrap_or(0)) {
                    let damage = self.device.active_screen_mut().erase_in_line(mode);
                    self.stage(damage);
                }
            }
            // CHT
            (None, b'I') => self
                .device
                .active_screen_mut()
                .move_forward_tabs(repeat_count(params.value(0))),
            // CBT
            (None, b'Z') => self
                .device
                .active_screen_mut()
                .move_backward_tabs(repeat_count(params.value(0))),
            // TBC
            (None, b'g') => {
                if let Some(edit) = CharacterTabEdit::from_tbc(params.value(0).unwrap_or(0)) {
                    self.device.active_screen_mut().edit_tab_stop(edit);
                }
            }
            // CTC
            (None, b'W') => {
                if let Some(edit) = CharacterTabEdit::from_ctc(params.value(0).unwrap_or(0)) {
                    self.device.active_screen_mut().edit_tab_stop(edit);
                }
            }
            // DECST8C
            (Some(b'?'), b'W') if params.value(0) == Some(5) => {
                self.device.active_screen_mut().reset_tab_stops()
            }
            // SGR
            (None, b'm') => {
                let pen = self.device.active_screen_mut().pen_mut();
                *pen = pen.applied(&params);
            }
            // DECSET
            (Some(b'?'), b'h') => self.set_private_modes(&params, true),
            // DECRST
            (Some(b'?'), b'l') => self.set_private_modes(&params, false),
            // DA2
            (Some(b'>'), b'c') if params.value(0).unwrap_or(0) == 0 => {
                self.reply(&secondary_attributes())
            }
            _ => {}
        }
    }

    // TODO: Implement the remaining OSC handlers — the palette (OSC 4 /
    // 10 / 11 / 12), the working directory (OSC 7), hyperlinks (OSC 8),
    // and the clipboard (OSC 52).
    fn osc_dispatch(&mut self, params: &[&[u8]]) {
        if let Some(title) = window_title(params) {
            self.device.set_title(Some(title.clone()));
            self.signal(VtSignal::Title(title));
        }
    }

    // TODO: Implement the APC webview verbs, which mint the placement
    // ids a `VtSignal::WebviewApc` carries.
    fn apc_dispatch(&mut self, _data: Vec<u8>) {}
}

/// The control functions an eight-bit C1 byte and its seven-bit `ESC`
/// form both request, so the two spellings cannot drift apart.
impl Executor<'_> {
    /// Moves the cursor down a row, scrolling at the bottom margin (IND,
    /// and the LF family that shares its effect).
    fn index(&mut self) {
        let damage = self.device.active_screen_mut().line_feed();
        self.stage(damage);
    }

    /// Returns the carriage and moves the cursor down a row (NEL).
    fn next_line(&mut self) {
        self.device.active_screen_mut().carriage_return();
        let damage = self.device.active_screen_mut().line_feed();
        self.stage(damage);
    }

    /// Moves the cursor up a row, scrolling at the top margin (RI).
    fn reverse_index(&mut self) {
        let damage = self.device.active_screen_mut().reverse_index();
        self.stage(damage);
    }

    /// Invokes a G code into GL until the next locking shift (the LS
    /// family).
    fn invoke_character_set(&mut self, g_code: GCode) {
        self.device.active_screen_mut().invoke_character_set(g_code);
    }

    /// Invokes a G code into GL for the next graphic character (SS2 and
    /// SS3).
    fn single_shift(&mut self, single_shift: SingleShift) {
        self.device.active_screen_mut().single_shift(single_shift);
    }

    /// Stages the reported damage and folds the result into the chunk
    /// liveness.
    fn stage(&mut self, damage: Option<DamageSpan>) {
        self.output.damaged |= self.tracker.stage_if_changed(damage);
    }

    /// Queues an out-of-band signal, preserving byte-stream order.
    ///
    /// Queuing a signal does not itself raise the chunk liveness; a
    /// signal whose effect is frame-relevant must stage its own damage.
    fn signal(&mut self, signal: VtSignal) {
        self.output.signals.push(signal);
    }

    /// Queues reply bytes for the owner to write back to the PTY.
    ///
    /// A reply changes nothing the renderer draws, so it deliberately
    /// leaves the chunk liveness alone.
    fn reply(&mut self, bytes: &[u8]) {
        self.output.replies.extend_from_slice(bytes);
    }

    /// Restores the most recently saved title, reporting the change
    /// (`XTWINOPS 23`).
    ///
    /// A pop that finds the stack empty reports nothing, and one that
    /// finds an entry holding no title reports the return to the host's
    /// default rather than an empty title.
    fn pop_title(&mut self) {
        let Some(restored) = self.device.pop_title() else {
            return;
        };
        self.device.set_title(restored.clone());
        match restored {
            Some(title) => self.signal(VtSignal::Title(title)),
            None => self.signal(VtSignal::ResetTitle),
        }
    }

    /// Returns every screen and mode to its power-up state (RIS),
    /// reporting the title's return to the host's default.
    ///
    /// A reset that finds no title reports nothing, so a terminal that
    /// was never titled does not wake the host.
    fn reset_device(&mut self) {
        let had_title = self.device.title().is_some();
        let damage = self.device.reset();
        self.stage(damage);
        if had_title {
            self.signal(VtSignal::ResetTitle);
        }
    }
}

/// The control functions a CSI sequence requests, where one final byte
/// stands for a list of independent settings.
impl Executor<'_> {
    /// Applies every private mode this terminal implements out of one
    /// `DECSET` or `DECRST` sequence, ignoring the numbers it does not.
    ///
    /// A sequence may carry several modes at once, and an unimplemented
    /// one must not hide an implemented one later in the list.
    fn set_private_modes(&mut self, params: &CsiParams<'_>, enabled: bool) {
        for mode in params.values().flatten() {
            match mode {
                // DECCKM
                1 => self.device.modes_mut().app_cursor = enabled,
                // DECOM
                6 => self
                    .device
                    .active_screen_mut()
                    .set_origin_mode(OriginMode::from_decset(enabled)),
                // Alternate screen
                47 if enabled => self.switch_to_alternate_screen(false),
                47 => {
                    self.switch_to_primary_screen();
                }
                // DECNKM
                66 => self.device.modes_mut().keypad_mode = KeypadMode::from_decset(enabled),
                // XTFOCUS
                1004 => self.device.modes_mut().focus_in_out = enabled,
                // Alternate scroll
                1007 => self.device.modes_mut().alternate_scroll = enabled,
                // Alternate screen with the cursor saved and restored
                1049 => self.set_alternate_screen_with_cursor(enabled),
                // Bracketed paste
                2004 => self.device.modes_mut().bracketed_paste = enabled,
                _ => self.set_mouse_mode(mode, enabled),
            }
        }
    }

    /// Applies a mouse tracking level or report encoding; a number
    /// neither answers is ignored.
    ///
    /// The numbers live on the two enums rather than here, so the
    /// tracking levels and the encodings each keep their mapping beside
    /// the type that models them.
    fn set_mouse_mode(&mut self, mode: u16, enabled: bool) {
        let modes = self.device.modes_mut();
        if let Some(tracking) = modes.mouse_tracking.with_decset(mode, enabled) {
            modes.mouse_tracking = tracking;
        } else if let Some(encoding) = modes.mouse_encoding.with_decset(mode, enabled) {
            modes.mouse_encoding = encoding;
        }
    }

    /// Applies `DECSET 1049` / `DECRST 1049`: a set saves the primary
    /// screen's cursor, flips, and erases the alternate screen; a reset
    /// flips back and restores that cursor.
    ///
    /// The set checks the active screen itself instead of trusting the
    /// flip helper's guard, because the save has to run before the flip
    /// and must not run at all when the alternate screen is already
    /// shown — it would overwrite that screen's DECSC slot. `DeviceState`
    /// has no primary-screen accessor, so "save on the primary" is
    /// expressible only while the primary is the active screen.
    ///
    /// The reset restores only when a flip happened, so a stray
    /// `DECRST 1049` on the primary screen leaves the cursor alone.
    fn set_alternate_screen_with_cursor(&mut self, enabled: bool) {
        if enabled {
            if self.device.modes().active_screen == ScreenKind::Alternate {
                return;
            }
            self.device.active_screen_mut().save_checkpoint();
            self.switch_to_alternate_screen(true);
        } else if self.switch_to_primary_screen() {
            self.device.active_screen_mut().restore_checkpoint();
        }
    }

    /// Shows the alternate screen, erasing it first when `erase` is
    /// set. Already showing it is a no-op: no repaint, no erase.
    ///
    /// The erase runs after the flip because it must reach the
    /// alternate screen, and `active_screen_mut` is the only way to a
    /// screen. It fills with the pen the alternate screen kept from its
    /// previous use, not the primary screen's — each screen owns its
    /// pen — which is a known departure from xterm's shared pen.
    ///
    /// # Invariants
    ///
    /// The flip and the staged `Full` are never separated by an early
    /// return: a frame after a screen flip must carry every viewport
    /// row, and `switch_screen` stages nothing itself.
    fn switch_to_alternate_screen(&mut self, erase: bool) {
        if self.device.modes().active_screen == ScreenKind::Alternate {
            return;
        }
        self.device.switch_screen(ScreenKind::Alternate);
        if erase {
            let damage = self
                .device
                .active_screen_mut()
                .erase_in_display(EraseScreenMode::All);
            self.stage(damage);
        }
        self.stage(Some(DamageSpan::Full));
    }

    /// Returns to the primary screen and names the placements the
    /// alternate screen owned; reports whether a flip happened. Already
    /// showing the primary screen is a no-op.
    ///
    /// The eviction is raised here rather than left to the owner's
    /// sweep because `switch_screen` takes the placements out of the
    /// table, so no later sweep can find them.
    ///
    /// # Invariants
    ///
    /// The flip and the staged `Full` are never separated by an early
    /// return, for the reason [`Self::switch_to_alternate_screen`]
    /// gives.
    fn switch_to_primary_screen(&mut self) -> bool {
        if self.device.modes().active_screen == ScreenKind::Primary {
            return false;
        }
        let placements = self.device.switch_screen(ScreenKind::Primary);
        if !placements.is_empty() {
            self.signal(VtSignal::WebviewEvicted { placements });
        }
        self.stage(Some(DamageSpan::Full));
        true
    }
}

/// A repeat count parameter, where an omitted or zero value means one.
///
/// ECMA-48 gives the default to an *empty* parameter only (§ 5.4.2 e);
/// an explicit zero selecting the default is ZERO DEFAULT MODE, which
/// its annex F deprecates. DEC spells the rule out per function instead
/// — VT220 states "a parameter of 0 or 1" for these counts — and that
/// is what applications expect, so a zero resolves to one here.
fn repeat_count(value: Option<u16>) -> u16 {
    match value {
        None | Some(0) => 1,
        Some(count) => count,
    }
}

/// The DA1 response: a VT102 with no extensions, the class alacritty
/// reports. A higher class would advertise features — Sixel, DRCS,
/// selective erase — this terminal does not implement.
const PRIMARY_ATTRIBUTES: &[u8] = b"\x1b[?6c";

/// Builds the DA2 response.
///
/// The terminal type is 0: xterm's table has no VT102 entry, and 0
/// ("VT100") is the only code that claims no VT220-and-up feature set.
/// The cartridge number is 1 rather than the zero xterm documents,
/// because this response follows alacritty byte for byte; real
/// terminals deviate here freely and readers ignore the field.
fn secondary_attributes() -> Vec<u8> {
    format!("\x1b[>0;{};1c", firmware_version()).into_bytes()
}

/// This crate's version, in the single number DA2's firmware field
/// carries.
fn firmware_version() -> u32 {
    fn component(value: &str) -> u32 {
        value
            .parse()
            .expect("cargo sets the version components from a parsed semver")
    }

    pack_version(
        component(env!("CARGO_PKG_VERSION_MAJOR")),
        component(env!("CARGO_PKG_VERSION_MINOR")),
        component(env!("CARGO_PKG_VERSION_PATCH")),
    )
}

/// Packs a version into the single number DA2's firmware field carries,
/// one hundred per component.
///
/// The encoding assumes every component stays below one hundred, which
/// this crate's versioning holds to; a minor or patch that reached it
/// would collide with the next component up.
fn pack_version(major: u32, minor: u32, patch: u32) -> u32 {
    major * 10_000 + minor * 100 + patch
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::color::{Color, Rgb};
    use crate::device::modes::{MouseEncoding, MouseTracking};
    use crate::frame::Frame;
    use crate::placement::{PlacementId, PlacementSize};
    use crate::screen::cell::Cell;
    use crate::screen::grid::GridSize;
    use crate::screen::grid::coords::GridColumn;
    use crate::screen::grid::run::Style;
    use crate::screen::viewport::ViewportLine;
    use crate::{OrzmaVt, Vt};

    /// Runs `chunk` through a fresh interpreter and hands back the
    /// device it wrote to together with everything the chunk produced.
    ///
    /// The interpreter is driven directly rather than through
    /// [`OrzmaVt`] because these tests read [`DeviceState`], which the
    /// [`Vt`] trait does not expose.
    fn interpret_fully(chunk: &[u8]) -> (DeviceState, InterpretOutput) {
        interpret_sized(4, chunk)
    }

    /// Runs `chunk` on a grid wide enough for the default tabulation
    /// stride: twenty columns put the right edge at 19, so the stops at
    /// 8 and 16 are reachable and the one at 24 is not.
    fn interpret_wide(chunk: &[u8]) -> DeviceState {
        interpret_sized(20, chunk).0
    }

    fn interpret_sized(cols: u16, chunk: &[u8]) -> (DeviceState, InterpretOutput) {
        let mut device = DeviceState::new(GridSize { cols, rows: 3 }, 10);
        let mut tracker = FrameTracker::new();
        let mut output = InterpretOutput::default();
        Interpreter::default().parse(&mut output, &mut device, &mut tracker, chunk);
        (device, output)
    }

    /// Runs `chunk` through a fresh interpreter and hands back the
    /// device it wrote to.
    fn interpret(chunk: &[u8]) -> DeviceState {
        interpret_fully(chunk).0
    }

    /// Reports the chunk liveness `chunk` produced from a fresh device.
    fn damage_of(chunk: &[u8]) -> bool {
        interpret_fully(chunk).1.damaged
    }

    /// Reports the reply bytes `chunk` produced, through the public
    /// entry point rather than the crate-internal interpreter.
    fn replies_of(chunk: &[u8]) -> Vec<u8> {
        let mut vt = OrzmaVt::new(GridSize { cols: 4, rows: 3 }, 10);
        vt.interpret(chunk).replies
    }

    /// One interpreter, device, and tracker kept across chunks, so a
    /// test can build up state with one chunk and then observe what a
    /// later chunk alone produced.
    struct Session {
        interpreter: Interpreter,
        device: DeviceState,
        tracker: FrameTracker,
    }

    impl Session {
        fn new() -> Self {
            Self {
                interpreter: Interpreter::default(),
                device: DeviceState::new(GridSize { cols: 4, rows: 3 }, 10),
                tracker: FrameTracker::new(),
            }
        }

        /// Interprets `chunk` and hands back what it alone produced.
        fn feed(&mut self, chunk: &[u8]) -> InterpretOutput {
            let mut output = InterpretOutput::default();
            self.interpreter
                .parse(&mut output, &mut self.device, &mut self.tracker, chunk);
            output
        }

        /// Emits the pending frame, if anything observable changed.
        fn frame(&mut self) -> Option<Frame> {
            self.tracker.emit(&self.device)
        }

        /// Mounts a one-cell placement at the active screen's cursor.
        fn mount(&mut self, view: &str) -> PlacementId {
            self.device
                .mount_placement(PlacementSize { rows: 1, cols: 1 }, view.to_string(), None)
                .expect("a mount under the cap is accepted")
        }

        fn active_screen(&self) -> ScreenKind {
            self.device.modes().active_screen
        }

        fn cursor_column(&self) -> u16 {
            self.device.active_screen().cursor_column().0
        }

        fn char_at(&self, line: u16, column: u16) -> char {
            self.device.active_screen().viewport_row(ViewportLine(line))[column].c
        }
    }

    /// Runs `setup` and then `chunk` over one session, and reports the
    /// liveness `chunk` alone produced.
    fn liveness_after(setup: &[u8], chunk: &[u8]) -> bool {
        let mut session = Session::new();
        session.feed(setup);
        session.feed(chunk).damaged
    }

    /// Asserts that staging row damage through the executor marks the
    /// chunk damaged.
    ///
    /// Case: a shell echoes one character, and the owner must open its
    /// coalesce window for the frame that repaints the row.
    #[test]
    fn staged_row_damage_marks_the_chunk_damaged() {
        assert!(damage_of(b"a"));
    }

    /// Asserts that both bells reach the signals and leave the chunk
    /// undamaged.
    ///
    /// Case: a shell rings the bell twice for an ambiguous completion,
    /// printing nothing.
    #[test]
    fn a_bell_reaches_the_signals() {
        let (_device, output) = interpret_fully(b"\x07\x07");
        assert_eq!(output.signals, vec![VtSignal::Bell, VtSignal::Bell]);
        assert!(!output.damaged);
    }

    /// Asserts that an OS command sets the window title and reports it.
    ///
    /// Case: a shell prompt sets the title before printing.
    #[test]
    fn a_title_sequence_reports_the_new_title() {
        let (_device, output) = interpret_fully(b"\x1b]0;hi\x07");
        assert_eq!(output.signals, vec![VtSignal::Title("hi".to_owned())]);
    }

    /// Asserts that setting a title leaves the chunk undamaged.
    ///
    /// Case: a prompt sets the title without printing anything, and the
    /// owner must not open a coalesce window for an empty frame.
    #[test]
    fn a_title_sequence_leaves_the_chunk_undamaged() {
        assert!(!damage_of(b"\x1b]0;hi\x07"));
    }

    /// Asserts that a saved title is restored by a pop.
    ///
    /// Case: a full-screen editor saves the shell's title, sets its
    /// own, and restores it on the way out.
    #[test]
    fn a_popped_title_is_restored() {
        let (device, output) =
            interpret_fully(b"\x1b]0;shell\x07\x1b[22t\x1b]0;editor\x07\x1b[23t");
        assert_eq!(
            output.signals,
            vec![
                VtSignal::Title("shell".to_owned()),
                VtSignal::Title("editor".to_owned()),
                VtSignal::Title("shell".to_owned()),
            ]
        );
        assert_eq!(device.title(), Some("shell"));
    }

    /// Asserts that popping an empty stack reports nothing.
    ///
    /// Case: a program restores a title it never saved.
    #[test]
    fn popping_an_empty_title_stack_reports_nothing() {
        let (_device, output) = interpret_fully(b"\x1b[23t");
        assert!(output.signals.is_empty());
    }

    /// Asserts that popping a title saved before any was set reports a
    /// reset rather than an empty title.
    ///
    /// Case: a program saves the title at startup, sets its own, and
    /// restores on exit, with the shell having set none.
    #[test]
    fn popping_an_unset_title_reports_a_reset() {
        let (device, output) = interpret_fully(b"\x1b[22t\x1b]0;editor\x07\x1b[23t");
        assert_eq!(
            output.signals,
            vec![VtSignal::Title("editor".to_owned()), VtSignal::ResetTitle]
        );
        assert_eq!(device.title(), None);
    }

    /// Asserts that a reset returns the title to its default and says
    /// so, rather than leaving the host showing a stale one.
    ///
    /// Case: the user runs `reset` after a program left a title behind.
    #[test]
    fn a_reset_reports_the_title_returning_to_its_default() {
        let (_device, output) = interpret_fully(b"\x1b]0;hi\x07\x1bc");
        assert_eq!(
            output.signals,
            vec![VtSignal::Title("hi".to_owned()), VtSignal::ResetTitle]
        );
    }

    /// Asserts that a reset with no title set reports nothing.
    ///
    /// Case: the user runs `reset` twice in a row.
    #[test]
    fn a_reset_without_a_title_reports_nothing() {
        let (_device, output) = interpret_fully(b"\x1bc");
        assert!(output.signals.is_empty());
    }

    /// Asserts that a title terminated by ST reaches the same handler
    /// as one terminated by BEL.
    ///
    /// Case: a program that emits the seven-bit string terminator sets
    /// the window title.
    #[test]
    fn a_string_terminator_ends_a_title_too() {
        let (_device, output) = interpret_fully(b"\x1b]0;hi\x1b\\");
        assert_eq!(output.signals, vec![VtSignal::Title("hi".to_owned())]);
    }

    /// Asserts that a window operation carrying a private marker does
    /// not reach the title stack.
    ///
    /// Case: an application sends `CSI > 22 t` to set the title
    /// modifier, which this terminal does not implement.
    #[test]
    fn a_private_window_operation_does_not_reach_the_title_stack() {
        let (_device, output) = interpret_fully(b"\x1b]0;hi\x07\x1b[>22t\x1b[23t");
        assert_eq!(output.signals, vec![VtSignal::Title("hi".to_owned())]);
    }

    /// Asserts that the raw C1 byte for RI reaches the screen.
    ///
    /// Case: a program emits an eight-bit reverse index on a terminal not
    /// running in UTF-8 mode.
    #[test]
    fn the_raw_c1_byte_reverse_indexes() {
        let device = interpret(b"a\r\x8d");
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(1))[0].c,
            'a'
        );
    }

    /// Asserts that the UTF-8 encoding of U+008D reaches the same arm.
    ///
    /// Case: a program running on a UTF-8 stream emits the reverse index.
    #[test]
    fn the_utf8_form_reverse_indexes() {
        let device = interpret(b"a\r\xc2\x8d");
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(1))[0].c,
            'a'
        );
    }

    /// Asserts that `ESC D` moves the cursor down a row and leaves the
    /// column where it stood.
    ///
    /// IND indexes and nothing more. The carriage return belongs to NEL
    /// alone, so IND must not be folded into the arm that pairs the two.
    ///
    /// Case: a full-screen program walks down one column of a form,
    /// emitting the seven-bit index between fields.
    #[test]
    fn the_seven_bit_index_keeps_the_column() {
        let device = interpret(b"a\x1bDb");
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(0))[0].c,
            'a'
        );
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(1))[1].c,
            'b'
        );
    }

    /// Asserts that `ESC E` moves the cursor down a row and returns the
    /// carriage.
    ///
    /// Case: a program ends a log line with the seven-bit next line
    /// instead of writing a CR and an LF of its own.
    #[test]
    fn the_seven_bit_next_line_returns_the_carriage() {
        let device = interpret(b"a\x1bEb");
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(0))[0].c,
            'a'
        );
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(1))[0].c,
            'b'
        );
    }

    /// Asserts that `ESC H` plants a tabulation stop at the cursor
    /// column.
    ///
    /// Case: a program sizes a column by walking the cursor to the width
    /// it wants, setting a stop there, and tabbing to it on later rows.
    #[test]
    fn the_seven_bit_tab_set_plants_a_stop_at_the_cursor() {
        let device = interpret(b"ab\x1bH\r\t");
        assert_eq!(device.active_screen().cursor_column(), GridColumn(2));
    }

    /// Asserts that `ESC 7` and `ESC 8` bracket a detour, putting the
    /// cursor back where the save found it.
    ///
    /// Case: a program saves its cursor, moves away to write a line
    /// elsewhere on the screen, restores, and continues where it left
    /// off.
    #[test]
    fn the_seven_bit_save_and_restore_bracket_a_detour() {
        let device = interpret(b"ab\x1b7\r\x1bDxy\x1b8c");
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(1))[0].c,
            'x'
        );
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(0))[2].c,
            'c'
        );
        assert_eq!(device.active_screen().cursor_column(), GridColumn(3));
    }

    /// Asserts that a scroll region set by `CSI r` is what a later
    /// linefeed scrolls against.
    ///
    /// Case: a full-screen application reserves the last row for a
    /// status line and fills the pane above it.
    #[test]
    fn a_scroll_region_reaches_the_linefeed() {
        let device = interpret(b"\x1b[1;2ra\n\nb");
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(1))[1].c,
            'b'
        );
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(2))[1].c,
            ' '
        );
    }

    /// Asserts that the flag-shaped private modes reach their fields
    /// on set and go back on reset.
    ///
    /// Case: a full-screen application turns on the modes it needs at
    /// startup and turns them off again on the way out.
    #[test]
    fn the_flag_private_modes_reach_their_fields() {
        let device = interpret(b"\x1b[?1;1004;1007;2004h");
        let modes = device.modes();
        assert!(modes.app_cursor);
        assert!(modes.focus_in_out);
        assert!(modes.alternate_scroll);
        assert!(modes.bracketed_paste);

        let device = interpret(b"\x1b[?1;1004;1007;2004h\x1b[?1;1004;1007;2004l");
        let modes = device.modes();
        assert!(!modes.app_cursor);
        assert!(!modes.focus_in_out);
        assert!(!modes.alternate_scroll);
        assert!(!modes.bracketed_paste);
    }

    /// Asserts that each mouse tracking number selects its own level,
    /// the levels replacing one another.
    ///
    /// Case: an editor raises its tracking from clicks to any-event
    /// motion when the user starts a drag selection.
    #[test]
    fn each_mouse_tracking_number_selects_its_level() {
        assert_eq!(
            interpret(b"\x1b[?1000h").modes().mouse_tracking,
            MouseTracking::Clicks
        );
        assert_eq!(
            interpret(b"\x1b[?1002h").modes().mouse_tracking,
            MouseTracking::Drag
        );
        assert_eq!(
            interpret(b"\x1b[?1000h\x1b[?1003h").modes().mouse_tracking,
            MouseTracking::Motion
        );
    }

    /// Asserts that resetting a tracking number that is not the active
    /// level leaves that level alone.
    ///
    /// Case: an application tears down every tracking mode it knows,
    /// including ones it never set, and must not disable the one it did.
    #[test]
    fn resetting_an_inactive_tracking_number_keeps_the_active_level() {
        assert_eq!(
            interpret(b"\x1b[?1002h\x1b[?1000l").modes().mouse_tracking,
            MouseTracking::Drag
        );
        assert_eq!(
            interpret(b"\x1b[?1002h\x1b[?1002l").modes().mouse_tracking,
            MouseTracking::Off
        );
    }

    /// Asserts that `DECSET 1006` selects SGR reports and its reset
    /// returns to the default framing.
    ///
    /// Case: an application asks for SGR reports so it can address a
    /// window wider than the legacy coordinate cap.
    #[test]
    fn the_sgr_mouse_number_selects_its_encoding() {
        assert_eq!(
            interpret(b"\x1b[?1006h").modes().mouse_encoding,
            MouseEncoding::Sgr
        );
        assert_eq!(
            interpret(b"\x1b[?1006h\x1b[?1006l").modes().mouse_encoding,
            MouseEncoding::X10
        );
    }

    /// Asserts that `DECSET 1005` is not answered, leaving the default
    /// framing in force.
    ///
    /// Case: an application asks for the UTF-8 coordinate extension,
    /// which `MouseReport::encode` does not implement — selecting it
    /// would report wrong coordinates past column 95.
    #[test]
    fn the_utf8_mouse_number_is_not_answered() {
        assert_eq!(
            interpret(b"\x1b[?1005h").modes().mouse_encoding,
            MouseEncoding::X10
        );
    }

    /// Asserts that an unknown private mode is ignored rather than
    /// disturbing the modes around it.
    ///
    /// Case: an application probes for a feature this terminal does not
    /// implement while other modes are already in force.
    #[test]
    fn an_unknown_private_mode_is_ignored() {
        let device = interpret(b"\x1b[?2004h\x1b[?9999h");
        assert!(device.modes().bracketed_paste);
    }

    /// Asserts that `CSI A` and `CSI B` move the cursor by whole rows
    /// and leave it in the column it was already in.
    ///
    /// Case: a full-screen application redraws a column of a table by
    /// stepping down it and back up.
    #[test]
    fn the_cursor_up_and_down_sequences_move_by_rows() {
        let device = interpret(b"\x1b[2;2H\x1b[1Bx\x1b[2Ay");
        let screen = device.active_screen();
        assert_eq!(screen.viewport_row(ViewportLine(2))[1].c, 'x');
        assert_eq!(screen.viewport_row(ViewportLine(0))[2].c, 'y');
    }

    /// Asserts that `CSI C` and `CSI D` move the cursor by whole
    /// columns in the same row.
    ///
    /// Case: a program spaces a label away from the left edge without
    /// emitting the blanks between.
    #[test]
    fn the_cursor_forward_and_back_sequences_move_by_columns() {
        let device = interpret(b"\x1b[2Cx\x1b[2Dy");
        let screen = device.active_screen();
        assert_eq!(screen.viewport_row(ViewportLine(0))[2].c, 'x');
        assert_eq!(screen.viewport_row(ViewportLine(0))[1].c, 'y');
    }

    /// Asserts that an omitted count moves one row, the default DEC
    /// gives every `Pn`.
    ///
    /// Case: a program emits the bare `CSI B` spelling to step down a
    /// single row.
    #[test]
    fn an_omitted_cursor_motion_count_moves_one_row() {
        let device = interpret(b"\x1b[Bx");
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(1))[0].c,
            'x'
        );
    }

    /// Asserts that `CSI E` moves down and returns to the first
    /// column, without scrolling the way `NEL` would.
    ///
    /// Case: a program starts the next record of a listing at the left
    /// edge two rows down.
    #[test]
    fn the_next_line_sequence_moves_down_and_returns_to_column_one() {
        let device = interpret(b"\x1b[1;3Hab\x1b[2Ex");
        let screen = device.active_screen();
        assert_eq!(screen.viewport_row(ViewportLine(0))[2].c, 'a');
        assert_eq!(screen.viewport_row(ViewportLine(2))[0].c, 'x');
    }

    /// Asserts that `CSI F` moves up and returns to the first column.
    ///
    /// Case: a program rewrites the heading two rows above the row it
    /// was filling.
    #[test]
    fn the_preceding_line_sequence_moves_up_and_returns_to_column_one() {
        let device = interpret(b"\x1b[3;3Hab\x1b[2Fx");
        let screen = device.active_screen();
        assert_eq!(screen.viewport_row(ViewportLine(2))[2].c, 'a');
        assert_eq!(screen.viewport_row(ViewportLine(0))[0].c, 'x');
    }

    /// Asserts that `CSI E` at the bottom margin stays put rather than
    /// scrolling the region.
    ///
    /// Case: a program emits a next-line at the foot of its pane, where
    /// `NEL` would have scrolled but `CNL` must not.
    #[test]
    fn the_next_line_sequence_does_not_scroll_at_the_bottom() {
        let device = interpret(b"a\x1b[3;1Hb\x1b[Ex");
        let screen = device.active_screen();
        assert_eq!(screen.viewport_row(ViewportLine(0))[0].c, 'a');
        assert_eq!(screen.viewport_row(ViewportLine(2))[0].c, 'x');
    }

    /// Asserts that a parameter list long enough to hit the parser's
    /// own cap loses its tail, which this terminal cannot detect.
    ///
    /// Case: an application sets nine attributes and two direct colours
    /// in one sequence, and the background never arrives.
    #[test]
    fn a_parameter_list_past_the_parser_cap_loses_its_tail() {
        let device = interpret(b"\x1b[0;1;2;3;4;5;7;8;9;38;2;255;0;0;48;2;0;0;255mx");
        let cell = device.active_screen().viewport_row(ViewportLine(0))[0];
        assert_eq!(cell.fg, Color::Rgb(Rgb { r: 255, g: 0, b: 0 }));
        assert_eq!(cell.bg, Color::DefaultBackground);
    }

    /// Asserts that `CSI m` reaches the pen, so a printed cell carries
    /// the attributes the sequence selected.
    ///
    /// Case: a build tool prints a red error message.
    #[test]
    fn the_select_graphic_rendition_sequence_reaches_the_pen() {
        let device = interpret(b"\x1b[31;1mx");
        let cell = device.active_screen().viewport_row(ViewportLine(0))[0];
        assert_eq!(cell.fg, Color::Indexed(1));
        assert!(cell.style.contains(Style::BOLD));
    }

    /// Asserts that the pen survives between sequences, so a run keeps
    /// its attributes until something changes them.
    ///
    /// Case: a program colours a word, prints it, and resets before the
    /// rest of the line.
    #[test]
    fn the_pen_survives_between_sequences() {
        let device = interpret(b"\x1b[31ma\x1b[mb");
        let row = device.active_screen().viewport_row(ViewportLine(0));
        assert_eq!(row[0].fg, Color::Indexed(1));
        assert_eq!(row[1].fg, Color::DefaultForeground);
    }

    /// Asserts that `CSI J` erases from the cursor to the end of the
    /// screen and leaves what precedes it.
    ///
    /// Case: a program finishes drawing a short menu and clears the
    /// stale rows a longer one left below it.
    #[test]
    fn the_erase_in_display_sequence_clears_below_the_cursor() {
        let device = interpret(b"ab\x1b[2;1Hcd\x1b[2;2H\x1b[J");
        let screen = device.active_screen();
        assert_eq!(screen.viewport_row(ViewportLine(0))[0].c, 'a');
        assert_eq!(screen.viewport_row(ViewportLine(1))[0].c, 'c');
        assert_eq!(screen.viewport_row(ViewportLine(1))[1].c, ' ');
    }

    /// Asserts that `CSI 2 J` clears the whole visible screen.
    ///
    /// Case: a full-screen application takes over and wipes whatever
    /// the shell left behind before its first paint.
    #[test]
    fn the_erase_in_display_sequence_clears_the_whole_screen() {
        let device = interpret(b"ab\x1b[2;1Hcd\x1b[2J");
        let screen = device.active_screen();
        assert_eq!(screen.viewport_row(ViewportLine(0))[0].c, ' ');
        assert_eq!(screen.viewport_row(ViewportLine(1))[0].c, ' ');
    }

    /// Asserts that an `ED` parameter this terminal does not answer
    /// leaves the screen alone rather than erasing something.
    ///
    /// Case: an application asks for `ED 3` to drop the scrollback,
    /// which this terminal does not model.
    #[test]
    fn an_unanswered_erase_in_display_parameter_erases_nothing() {
        let device = interpret(b"ab\x1b[3J");
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(0))[0].c,
            'a'
        );
    }

    /// Asserts that `CSI K` erases from the cursor to the end of the
    /// row and leaves the rows around it.
    ///
    /// Case: a shell redraws a prompt line after the user deletes the
    /// tail of what they typed.
    #[test]
    fn the_erase_in_line_sequence_clears_to_the_end_of_the_row() {
        let device = interpret(b"abc\x1b[1;2H\x1b[K");
        let screen = device.active_screen();
        assert_eq!(screen.viewport_row(ViewportLine(0))[0].c, 'a');
        assert_eq!(screen.viewport_row(ViewportLine(0))[1].c, ' ');
    }

    /// Asserts that `CSI 2 K` clears the whole row the cursor sits on.
    ///
    /// Case: a status line is rewritten from scratch each time its
    /// contents change.
    #[test]
    fn the_erase_in_line_sequence_clears_the_whole_row() {
        let device = interpret(b"abc\x1b[1;2H\x1b[2K");
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(0))[0].c,
            ' '
        );
    }

    /// Asserts that `CSI I` advances the cursor by whole tab stops.
    ///
    /// Case: a program lays out a table by asking for two tab stops
    /// rather than emitting two horizontal tabs.
    #[test]
    fn the_forward_tabulation_sequence_advances_by_stops() {
        let device = interpret_wide(b"\x1b[2Ix");
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(0))[16].c,
            'x'
        );
    }

    /// Asserts that `CSI Z` walks the cursor back by whole tab stops.
    ///
    /// Case: a program aligning a column overshoots and steps back one
    /// stop to line up with the header above it.
    #[test]
    fn the_backward_tabulation_sequence_retreats_by_stops() {
        let device = interpret_wide(b"\x1b[1;20H\x1b[Zx");
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(0))[16].c,
            'x'
        );
    }

    /// Asserts that an omitted tabulation count moves one stop, the
    /// default every `Pn` carries.
    ///
    /// Case: a program emits the bare `CSI I` spelling for a single
    /// tab.
    #[test]
    fn an_omitted_tabulation_count_moves_one_stop() {
        let device = interpret_wide(b"\x1b[Ix");
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(0))[8].c,
            'x'
        );
    }

    /// Asserts that a zero tabulation count moves one stop rather than
    /// standing still.
    ///
    /// Case: a program computes its tab count and emits `CSI 0 I` when
    /// the computation yields nothing to skip.
    #[test]
    fn a_zero_tabulation_count_moves_one_stop() {
        let device = interpret_wide(b"\x1b[0Ix");
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(0))[8].c,
            'x'
        );
    }

    /// Asserts that `CSI 3 g` clears every tab stop, so a later tab
    /// runs to the right edge.
    ///
    /// Case: a program installs its own column layout and clears the
    /// default eight-column stride first.
    #[test]
    fn the_tabulation_clear_sequence_clears_every_stop() {
        let device = interpret_wide(b"\x1b[3g\tx");
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(0))[19].c,
            'x'
        );
    }

    /// Asserts that `CSI 0 W` sets a tab stop at the cursor column.
    ///
    /// Case: a program installs a stop with the cursor-tabulation
    /// spelling rather than `HTS`.
    #[test]
    fn the_cursor_tabulation_control_sequence_sets_a_stop() {
        let device = interpret_wide(b"\x1b[3g\x1b[1;4H\x1b[0W\x1b[1;1H\tx");
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(0))[3].c,
            'x'
        );
    }

    /// Asserts that `CSI ? 5 W` reinstalls the default eight-column
    /// stride.
    ///
    /// Case: a program clears every stop, lays out its own table, and
    /// restores the defaults before handing the terminal back.
    #[test]
    fn the_tab_stop_reset_sequence_reinstalls_the_default_stride() {
        let device = interpret_wide(b"\x1b[3g\x1b[?5W\tx");
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(0))[8].c,
            'x'
        );
    }

    /// Asserts that `CSI H` addresses the cursor.
    ///
    /// Case: a full-screen application jumps to the second row and
    /// second column to draw a box corner.
    #[test]
    fn the_cursor_position_sequence_addresses_the_cursor() {
        let device = interpret(b"\x1b[2;2Hx");
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(1))[1].c,
            'x'
        );
    }

    /// Asserts that `CSI f` addresses the cursor the same way `CSI H`
    /// does.
    ///
    /// Case: an older program uses the horizontal-and-vertical-position
    /// spelling it was written against.
    #[test]
    fn the_position_sequence_matches_cursor_position() {
        let device = interpret(b"\x1b[2;2fx");
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(1))[1].c,
            'x'
        );
    }

    /// Asserts that origin mode moves the cursor-addressing origin to
    /// the top margin.
    ///
    /// Case: an application reserves a header row, turns on origin mode,
    /// and addresses the first row of its own pane.
    #[test]
    fn origin_mode_moves_the_addressing_origin() {
        let device = interpret(b"\x1b[2;3r\x1b[?6h\x1b[1;1Hx");
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(1))[0].c,
            'x'
        );
    }

    /// Asserts that `CSI ? 6 l` seats the cursor at the upper-left
    /// corner.
    ///
    /// Case: a full-screen application drops origin mode on its way out
    /// and prints without addressing the cursor first.
    #[test]
    fn resetting_origin_mode_seats_the_cursor_at_the_corner() {
        let device = interpret(b"\x1b[2;3r\x1b[?6h\x1b[?6lx");
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(0))[0].c,
            'x'
        );
    }

    /// Asserts that a private mode this terminal does not implement does
    /// not hide one it does.
    ///
    /// Case: an application turns on application cursor keys and origin
    /// mode in a single `CSI ? 1 ; 6 h`.
    #[test]
    fn an_unimplemented_private_mode_does_not_hide_origin_mode() {
        let device = interpret(b"\x1b[2;3r\x1b[?1;6h\x1b[1;1Hx");
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(1))[0].c,
            'x'
        );
    }

    /// Asserts that `ESC =` puts the keypad in application mode.
    ///
    /// Case: a full-screen editor starts up and takes the numeric keypad
    /// over so that its own bindings receive those keys.
    #[test]
    fn keypad_application_mode_selects_application_sequences() {
        let device = interpret(b"\x1b=");
        assert_eq!(device.modes().keypad_mode, KeypadMode::Application);
    }

    /// Asserts that `ESC >` puts the keypad back in numeric mode.
    ///
    /// Case: a full-screen editor exits and hands the keypad back to the
    /// shell, where the digit keys must type digits again.
    #[test]
    fn keypad_numeric_mode_selects_ascii_numerals() {
        let device = interpret(b"\x1b=\x1b>");
        assert_eq!(device.modes().keypad_mode, KeypadMode::Numeric);
    }

    /// Asserts that `CSI ? 66 h` selects application mode, the same
    /// state `ESC =` selects.
    ///
    /// Case: an application that manages its modes through the request
    /// and report pair takes the numeric keypad over as it starts up.
    #[test]
    fn the_numeric_keypad_mode_set_matches_keypad_application_mode() {
        let device = interpret(b"\x1b[?66h");
        assert_eq!(device.modes().keypad_mode, KeypadMode::Application);
    }

    /// Asserts that `CSI ? 66 l` selects numeric mode, the same state
    /// `ESC >` selects.
    ///
    /// Case: the same application hands the numeric keypad back as it
    /// shuts down.
    #[test]
    fn the_numeric_keypad_mode_reset_matches_keypad_numeric_mode() {
        let device = interpret(b"\x1b=\x1b[?66l");
        assert_eq!(device.modes().keypad_mode, KeypadMode::Numeric);
    }

    /// Asserts that `ESC c` blanks every visible row.
    ///
    /// Case: a program dies mid-redraw and leaves the screen unusable,
    /// so the user runs `reset` to take the terminal back.
    #[test]
    fn the_seven_bit_reset_blanks_every_visible_row() {
        let device = interpret(b"ab\r\nc\x1bc");
        for line in 0..3 {
            let row = device.active_screen().viewport_row(ViewportLine(line));
            assert!(row.iter().all(|cell| *cell == Cell::default()));
        }
    }

    /// Asserts that the repaint `ESC c` calls for reaches the chunk
    /// liveness rather than being dropped by the handler.
    ///
    /// Case: the user runs `reset` on a screen a previous command filled,
    /// and the owner must open its coalesce window for the frame that
    /// repaints it.
    #[test]
    fn the_seven_bit_reset_marks_its_own_chunk_damaged() {
        assert!(liveness_after(b"a", b"\x1bc"));
    }

    /// Asserts that a primary device attributes request reports the
    /// terminal's architectural class.
    ///
    /// Case: an application probes the terminal's capabilities at
    /// startup and blocks until the class arrives.
    #[test]
    fn a_primary_attributes_request_reports_the_terminal_class() {
        assert_eq!(replies_of(b"\x1b[c"), b"\x1b[?6c");
    }

    /// Asserts that an explicit zero requests the same class an omitted
    /// parameter does.
    ///
    /// Case: a program that builds its sequences from an unset integer
    /// variable emits `CSI 0 c`.
    #[test]
    fn an_explicit_zero_requests_the_same_class() {
        assert_eq!(replies_of(b"\x1b[0c"), b"\x1b[?6c");
    }

    /// Asserts that a nonzero parameter requests nothing.
    ///
    /// Case: an application sends a device attributes variant this
    /// terminal does not answer.
    #[test]
    fn a_nonzero_parameter_requests_nothing() {
        assert!(replies_of(b"\x1b[1c").is_empty());
    }

    /// Asserts that the seven-bit identify reports the primary class.
    ///
    /// Case: an application written for a VT100 probes the terminal
    /// with the obsolete `ESC Z` spelling.
    #[test]
    fn the_seven_bit_identify_reports_the_primary_class() {
        assert_eq!(replies_of(b"\x1bZ"), b"\x1b[?6c");
    }

    /// Asserts that the raw C1 byte for DECID reports the primary
    /// class.
    ///
    /// Case: a program emits an eight-bit identify on a terminal not
    /// running in UTF-8 mode.
    #[test]
    fn the_raw_c1_identify_reports_the_primary_class() {
        assert_eq!(replies_of(b"\x9a"), b"\x1b[?6c");
    }

    /// Asserts that the UTF-8 encoding of U+009A reaches the same arm.
    ///
    /// Case: a program running on a UTF-8 stream emits the identify.
    #[test]
    fn the_utf8_form_identifies_the_terminal() {
        assert_eq!(replies_of(b"\xc2\x9a"), b"\x1b[?6c");
    }

    /// Asserts that a reply leaves the chunk undamaged, so the owner
    /// does not open a coalesce window for a frame with nothing in it.
    ///
    /// Case: an application probes the terminal while the screen sits
    /// untouched at a prompt.
    #[test]
    fn a_reply_leaves_the_chunk_undamaged() {
        assert!(!damage_of(b"\x1b[c"));
    }

    /// Asserts that two requests in one chunk both reach the replies,
    /// concatenated in the order they arrived.
    ///
    /// Case: an application flushes its whole capability probe in a
    /// single write.
    #[test]
    fn replies_accumulate_within_one_chunk() {
        assert_eq!(replies_of(b"\x1b[c\x1b[c"), b"\x1b[?6c\x1b[?6c");
    }

    /// Asserts that a secondary device attributes request reports the
    /// terminal type and a firmware level.
    ///
    /// Case: an application checks which terminal it is talking to
    /// before enabling a version-gated workaround.
    #[test]
    fn a_secondary_attributes_request_reports_the_firmware_level() {
        let reply = replies_of(b"\x1b[>c");
        let version = reply
            .strip_prefix(b"\x1b[>0;".as_slice())
            .and_then(|rest| rest.strip_suffix(b";1c".as_slice()))
            .expect("the reply is framed as CSI > 0 ; Pv ; 1 c");
        assert!(version.iter().all(u8::is_ascii_digit));
        assert!(version.iter().any(|digit| *digit != b'0'));
    }

    /// Asserts that an explicit zero requests the same secondary
    /// attributes an omitted parameter does.
    ///
    /// Case: a program that builds its sequences from an unset integer
    /// variable emits `CSI > 0 c`.
    #[test]
    fn an_explicit_zero_requests_the_same_secondary_attributes() {
        let reply = replies_of(b"\x1b[>0c");
        assert!(!reply.is_empty());
        assert_eq!(reply, replies_of(b"\x1b[>c"));
    }

    /// Asserts that a nonzero parameter requests no secondary
    /// attributes.
    ///
    /// Case: an application sends a secondary attributes variant this
    /// terminal does not answer.
    #[test]
    fn a_nonzero_parameter_requests_no_secondary_attributes() {
        assert!(replies_of(b"\x1b[>1c").is_empty());
    }

    /// Asserts that a tertiary device attributes request is ignored.
    ///
    /// Case: an application asks for the terminal unit id, which this
    /// terminal never reports, and falls back after its own timeout.
    #[test]
    fn a_tertiary_attributes_request_is_ignored() {
        assert!(replies_of(b"\x1b[=c").is_empty());
    }

    /// Asserts that a version packs one hundred per component.
    ///
    /// Case: a release bumps the minor version and the firmware level
    /// DA2 reports has to move with it.
    #[test]
    fn a_version_packs_one_hundred_per_component() {
        assert_eq!(pack_version(1, 2, 3), 10_203);
    }

    /// Asserts that `ESC # 8` fills every visible row with the alignment
    /// pattern.
    ///
    /// Case: a service technician sends the alignment pattern to judge
    /// the geometry of a display showing a half-drawn prompt.
    #[test]
    fn the_alignment_pattern_fills_every_visible_row() {
        let device = interpret(b"ab\r\nc\x1b#8");
        for line in 0..3 {
            let row = device.active_screen().viewport_row(ViewportLine(line));
            assert!(row.iter().all(|cell| cell.c == 'E'));
        }
    }

    /// Asserts that the repaint `ESC # 8` calls for reaches the chunk
    /// liveness rather than being dropped by the handler.
    ///
    /// Case: the technician sends the alignment pattern, and the owner
    /// must open its coalesce window for the frame that repaints the
    /// screen.
    #[test]
    fn the_alignment_pattern_marks_its_own_chunk_damaged() {
        assert!(liveness_after(b"a", b"\x1b#8"));
    }

    /// Asserts that a control function this terminal does not implement
    /// is ignored rather than fatal.
    ///
    /// The agreed policy follows what VT terminals do with sequences
    /// they do not implement. It is also the point of the dispatcher:
    /// before it existed every CSI sequence reached a `todo!()`.
    ///
    /// Case: a program inserts blanks with `ICH` on a terminal that has
    /// no character-editing functions yet.
    #[test]
    fn an_unimplemented_sequence_is_ignored() {
        let device = interpret(b"\x1b[2@a");
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(0))[0].c,
            'a'
        );
    }

    /// Asserts that an operating system command is ignored rather than
    /// fatal, and that the parser returns to ground behind it.
    ///
    /// Case: a shell prompt sets the window title before printing, on a
    /// terminal whose OSC handlers have not landed yet.
    #[test]
    fn a_title_sequence_is_ignored_rather_than_fatal() {
        let device = interpret(b"\x1b]0;hi\x07a");
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(0))[0].c,
            'a'
        );
    }

    /// Asserts that an application program command is ignored rather
    /// than fatal, and that the parser returns to ground behind it.
    ///
    /// Case: a program probes for the webview verbs on a terminal whose
    /// APC handler has not landed yet.
    #[test]
    fn an_application_program_command_is_ignored_rather_than_fatal() {
        let device = interpret(b"\x1b_hi\x1b\\a");
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(0))[0].c,
            'a'
        );
    }

    /// Asserts that a device control string is ignored rather than
    /// fatal, and that the parser returns to ground behind it.
    ///
    /// Case: an application opens a Sixel image on a terminal that has
    /// no DCS handlers.
    #[test]
    fn a_device_control_string_is_ignored_rather_than_fatal() {
        let device = interpret(b"\x1bP0q\x1b\\a");
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(0))[0].c,
            'a'
        );
    }

    /// Asserts that a sequence carrying an intermediate does not reach
    /// the control function that shares its final byte.
    ///
    /// The agreed policy refuses every intermediate rather than
    /// whitelisting the ones this terminal implements. `vtparse` promotes
    /// intermediates into the parameter slice, so DECCARA and DECSTBM
    /// reach the dispatcher with the same final byte and differ only in
    /// that trailing byte.
    ///
    /// Case: an application changes the attributes of a rectangle with
    /// `CSI 1 ; 2 $ r`.
    #[test]
    fn an_intermediate_does_not_reach_the_scroll_region() {
        let device = interpret(b"\x1b[1;2$ra\n\nb");
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(2))[1].c,
            'b'
        );
    }

    /// Asserts that `ESC M` scrolls the region down when the cursor
    /// already sits on the top margin.
    ///
    /// Case: a pager walks backwards through a document with the
    /// seven-bit reverse index while the cursor rests on the first row.
    #[test]
    fn the_seven_bit_reverse_index_scrolls_at_the_top_margin() {
        let device = interpret(b"a\r\x1bM");
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(1))[0].c,
            'a'
        );
    }

    /// Asserts that a set designated into G0 maps the characters
    /// printed after it.
    ///
    /// Case: a program draws a horizontal rule by designating DEC
    /// Special Graphics into G0 and printing `q`.
    #[test]
    fn a_set_designated_into_g0_maps_what_follows() {
        let device = interpret(b"\x1b(0q");
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(0))[0].c,
            '─'
        );
    }

    /// Asserts that designating ASCII over a G code restores letters.
    ///
    /// Case: a program finishes a box and emits `ESC ( B` so the next
    /// `q` prints as a letter again.
    #[test]
    fn redesignating_ascii_restores_letters() {
        let device = interpret(b"\x1b(0\x1b(Bq");
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(0))[0].c,
            'q'
        );
    }

    /// Asserts that a final with no set behind it designates ASCII
    /// rather than leaving the previous set in force.
    ///
    /// The agreed policy is to fall back rather than drop the sequence:
    /// leaving DEC Special Graphics designated would print the
    /// application's text as line segments.
    ///
    /// Case: a program draws a box, then designates the Finnish
    /// national replacement set with `ESC ( C` before writing a label.
    #[test]
    fn an_unsupported_designation_falls_back_to_ascii() {
        let device = interpret(b"\x1b(0\x1b(Cq");
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(0))[0].c,
            'q'
        );
    }

    /// Asserts that a designation whose final takes two bytes reaches
    /// the same ASCII fallback a one-byte final does.
    ///
    /// Case: a program draws a box with line drawing, then designates
    /// the Greek supplemental set with `ESC ( " >` before writing a
    /// label.
    #[test]
    fn a_two_byte_final_designation_falls_back_to_ascii() {
        let device = interpret(b"\x1b(0\x1b(\">q");
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(0))[0].c,
            'q'
        );
    }

    /// Asserts that `SO` invokes G1 into GL and `SI` returns G0 to it.
    ///
    /// Case: a program designates line drawing into G1 once, then
    /// brackets each run of box characters with `SO` and `SI` instead of
    /// redesignating G0 every time.
    #[test]
    fn the_shift_out_and_shift_in_pair_swaps_the_invoked_set() {
        let device = interpret(b"\x1b)0\x0eq\x0fq");
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(0))[0].c,
            '─'
        );
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(0))[1].c,
            'q'
        );
    }

    /// Asserts that `ESC n` invokes G2 into GL for everything that
    /// follows.
    ///
    /// Case: a program parks line drawing in G2 and locks it into GL
    /// once, leaving G0 free to hold the set its text is written in.
    #[test]
    fn the_locking_shift_two_invokes_g2() {
        let device = interpret(b"\x1b*0q\x1bnq");
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(0))[0].c,
            'q'
        );
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(0))[1].c,
            '─'
        );
    }

    /// Asserts that `ESC o` invokes G3 into GL for everything that
    /// follows.
    ///
    /// Case: a program that already uses G2 parks a second set in G3 and
    /// locks that one into GL instead.
    #[test]
    fn the_locking_shift_three_invokes_g3() {
        let device = interpret(b"\x1b+0q\x1boq");
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(0))[0].c,
            'q'
        );
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(0))[1].c,
            '─'
        );
    }

    /// Asserts that the seven-bit `SS2` maps one character and then
    /// stops applying.
    ///
    /// Case: a program prints one box character mid-sentence with
    /// `ESC N` rather than shifting GL and shifting it back.
    #[test]
    fn the_seven_bit_single_shift_two_lasts_one_character() {
        let device = interpret(b"\x1b*0\x1bNqq");
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(0))[0].c,
            '─'
        );
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(0))[1].c,
            'q'
        );
    }

    /// Asserts that the raw C1 byte for SS2 reaches the same arm.
    ///
    /// Case: a program emits an eight-bit single shift on a terminal not
    /// running in UTF-8 mode.
    #[test]
    fn the_raw_c1_byte_single_shifts() {
        let device = interpret(b"\x1b*0\x8eqq");
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(0))[0].c,
            '─'
        );
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(0))[1].c,
            'q'
        );
    }

    /// Asserts that the UTF-8 encoding of U+008E reaches the same arm.
    ///
    /// Case: a program running on a UTF-8 stream emits the single shift.
    #[test]
    fn the_utf8_form_single_shifts() {
        let device = interpret(b"\x1b*0\xc2\x8eqq");
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(0))[0].c,
            '─'
        );
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(0))[1].c,
            'q'
        );
    }

    /// Asserts that DEL neither reaches a cell nor advances the cursor.
    ///
    /// vtparse hands DEL to the actor with the rest of GL and leaves the
    /// decision here. Every set in this terminal's repertoire holds 94
    /// characters, for which DEL displays nothing; a 96-character set
    /// would make it printable, and that decision would then belong to
    /// the character set mapping.
    ///
    /// Case: a program pads a fixed-width record with DEL, as a paper
    /// tape editor does to strike a character out.
    #[test]
    fn delete_prints_nothing() {
        let device = interpret(b"a\x7fb");
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(0))[0].c,
            'a'
        );
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(0))[1].c,
            'b'
        );
    }

    /// Asserts that DEL does not spend a pending single shift.
    ///
    /// Case: a program pads with DEL between emitting `SS2` and the box
    /// character the shift was meant for.
    #[test]
    fn delete_leaves_a_pending_single_shift_armed() {
        let device = interpret(b"\x1b*0\x1bN\x7fq");
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(0))[0].c,
            '─'
        );
    }

    /// Asserts that a sequence whose intermediate falls out of the
    /// parameter slice through `vtparse`'s own parameter limit still does
    /// not reach the control function that shares its final byte.
    ///
    /// The agreed policy checks `parameters_truncated` as its own guard
    /// rather than trusting `has_intermediates()` alone to catch every
    /// intermediate the parser drops. `vtparse` promotes a trailing
    /// intermediate into the parameter slice only while `num_params` has
    /// room under its own 32-parameter limit; once a sequence's own
    /// parameters already fill every slot, the promotion is refused and
    /// the loss is reported through `parameters_truncated` instead, so
    /// the intermediate never lands in the slice for `has_intermediates()`
    /// to see. A dispatcher that trusted `has_intermediates()` alone would
    /// read such a sequence as a bare `CSI 1 ; 2 r` and apply it as
    /// DECSTBM instead of refusing it as the DECCARA-shaped sequence it
    /// is.
    ///
    /// Case: an application changes the attributes of a rectangle with
    /// `CSI 1 ; 2 $ r`, sent with a parameter list long enough to exhaust
    /// `vtparse`'s own 32-parameter limit before the trailing `$`
    /// arrives.
    #[test]
    fn a_truncated_intermediate_does_not_reach_the_scroll_region() {
        let chunk = format!("\x1b[1;2{}$ra\n\nb", ";".repeat(29));
        let device = interpret(chunk.as_bytes());
        assert_eq!(
            device.active_screen().viewport_row(ViewportLine(2))[1].c,
            'b'
        );
    }

    /// Asserts that `?47h` shows the alternate screen and repaints the
    /// whole viewport, leaving the primary screen's contents in place
    /// behind it.
    ///
    /// Case: a program built against the old termcap pair opens on the
    /// alternate screen while the shell's prompt sits on the primary.
    #[test]
    fn decset_47_shows_the_alternate_screen() {
        let mut session = Session::new();
        session.feed(b"a");
        session.frame();
        let output = session.feed(b"\x1b[?47h");
        assert_eq!(session.active_screen(), ScreenKind::Alternate);
        assert!(output.damaged);
        assert_eq!(session.char_at(0, 0), ' ');
        let frame = session.frame().expect("a flip emits a full frame");
        assert_eq!(frame.rows.len(), 3);
    }

    /// Asserts that `?47l` returns to the primary screen with its
    /// contents intact, repaints the whole viewport, and raises no
    /// eviction when the alternate screen held no placements.
    ///
    /// Case: the program exits and the shell's prompt from before it
    /// must reappear.
    #[test]
    fn decrst_47_returns_to_the_primary_screen() {
        let mut session = Session::new();
        session.feed(b"a\x1b[?47h");
        session.frame();
        let output = session.feed(b"\x1b[?47l");
        assert_eq!(session.active_screen(), ScreenKind::Primary);
        assert!(output.damaged);
        assert!(output.signals.is_empty());
        assert_eq!(session.char_at(0, 0), 'a');
        let frame = session.frame().expect("the flip back emits a full frame");
        assert_eq!(frame.rows.len(), 3);
    }

    /// Asserts that a DECSET already on the alternate screen and a
    /// DECRST already on the primary screen are complete no-ops rather
    /// than repaints.
    ///
    /// Case: a wrapper script runs a program's `rmcup` string although
    /// the program never got to send `smcup`, and later a program
    /// re-sends its initialisation string while already full-screen.
    #[test]
    fn a_redundant_alternate_screen_switch_does_nothing() {
        let mut session = Session::new();
        let output = session.feed(b"\x1b[?47l");
        assert!(!output.damaged);
        assert!(output.signals.is_empty());
        assert_eq!(session.active_screen(), ScreenKind::Primary);

        session.feed(b"\x1b[?47h");
        let output = session.feed(b"\x1b[?47h");
        assert!(!output.damaged);
        assert!(output.signals.is_empty());
        assert_eq!(session.active_screen(), ScreenKind::Alternate);
    }

    /// Asserts that leaving the alternate screen names its placements
    /// in the chunk's own signals and leaves the primary screen's
    /// placement in the next frame.
    ///
    /// Case: a full-screen program that mounted a webview exits, and
    /// the shell's own webview from before it must survive.
    #[test]
    fn leaving_the_alternate_screen_evicts_only_its_placements() {
        let mut session = Session::new();
        let kept = session.mount("shell");
        session.feed(b"\x1b[?47h");
        let dropped = session.mount("app");
        let output = session.feed(b"\x1b[?47l");
        assert_eq!(
            output.signals,
            vec![VtSignal::WebviewEvicted {
                placements: vec![dropped]
            }]
        );
        let frame = session.frame().expect("the flip back emits");
        let listed: Vec<PlacementId> = frame
            .placements
            .expect("a placement change is listed")
            .iter()
            .map(|placement| placement.id)
            .collect();
        assert_eq!(listed, vec![kept]);
    }

    /// Asserts that `?1049h` shows the alternate screen erased, whatever
    /// the previous full-screen program left on it.
    ///
    /// Case: vim starts after a program that used the bare `?47` pair
    /// exited with its last frame still on the alternate screen.
    #[test]
    fn decset_1049_erases_the_alternate_screen() {
        let mut session = Session::new();
        session.feed(b"\x1b[?47hx\x1b[?47l");
        let output = session.feed(b"\x1b[?1049h");
        assert_eq!(session.active_screen(), ScreenKind::Alternate);
        assert!(output.damaged);
        assert_eq!(session.char_at(0, 0), ' ');
    }

    /// Asserts that `?1049l` leaves the alternate screen's contents in
    /// place rather than erasing them on the way out.
    ///
    /// Case: vim exits, and a later program enters with the bare `?47h`
    /// and finds vim's last frame still there, as it would under xterm.
    #[test]
    fn decrst_1049_does_not_erase_the_alternate_screen() {
        let mut session = Session::new();
        session.feed(b"\x1b[?1049hx\x1b[?1049l");
        session.feed(b"\x1b[?47h");
        assert_eq!(session.char_at(0, 0), 'x');
    }

    /// Asserts that `?1049h` saves the cursor into the primary screen's
    /// own DECSC slot, so a later `ESC 8` on the primary finds the
    /// position the flip saved rather than the shell's earlier save.
    ///
    /// Case: a shell saves its cursor with `ESC 7`, runs a full-screen
    /// program whose `?1049h` overwrites that save, and restores with
    /// `ESC 8` after the program exits.
    #[test]
    fn decset_1049_saves_the_cursor_into_the_primary_checkpoint() {
        let mut session = Session::new();
        session.feed(b"\x1b[1;2H\x1b7\x1b[1;4H\x1b[?1049h\x1b[?1049l\x1b8");
        assert_eq!(session.cursor_column(), 3);
    }

    /// Asserts that `?1049l` restores the cursor `?1049h` saved, moving
    /// it from wherever the primary screen's cursor was left in between.
    ///
    /// Case: a program enters with `?1049h`, drops back to the primary
    /// screen with `?47l` to print a line, returns with `?47h`, and
    /// finally exits with `?1049l`.
    #[test]
    fn decrst_1049_restores_the_saved_primary_cursor() {
        let mut session = Session::new();
        session.feed(b"\x1b[1;2H\x1b[?1049h\x1b[?47l\x1b[1;4H\x1b[?47h");
        let output = session.feed(b"\x1b[?1049l");
        assert_eq!(session.active_screen(), ScreenKind::Primary);
        assert!(output.damaged);
        assert_eq!(session.cursor_column(), 1);
    }

    /// Asserts that `?1049h` while already on the alternate screen does
    /// nothing: no repaint, no signal, and the alternate screen's own
    /// DECSC slot left alone.
    ///
    /// Case: a program re-sends its terminal initialisation string
    /// while it is already running full-screen.
    #[test]
    fn a_redundant_decset_1049_leaves_the_alternate_checkpoint_alone() {
        let mut session = Session::new();
        session.feed(b"\x1b[?1049h\x1b[1;2H\x1b7\x1b[1;4H");
        assert_eq!(session.active_screen(), ScreenKind::Alternate);
        let output = session.feed(b"\x1b[?1049h");
        assert!(!output.damaged);
        assert!(output.signals.is_empty());
        assert_eq!(session.char_at(0, 0), ' ');
        session.feed(b"\x1b8");
        assert_eq!(session.cursor_column(), 1);
    }

    /// Asserts that `?1049l` while already on the primary screen does
    /// nothing, not even the DECRC.
    ///
    /// Case: a wrapper script runs a program's `rmcup` string although
    /// the program was killed before it sent `smcup`.
    #[test]
    fn a_redundant_decrst_1049_does_not_restore_the_cursor() {
        let mut session = Session::new();
        session.feed(b"\x1b[1;2H\x1b7\x1b[1;4H");
        let output = session.feed(b"\x1b[?1049l");
        assert!(!output.damaged);
        assert!(output.signals.is_empty());
        assert_eq!(session.cursor_column(), 3);
    }

    /// Asserts that `?1049l` after a bare `?47h` restores whatever the
    /// primary screen's DECSC slot holds, which is the home position
    /// when nothing was ever saved.
    ///
    /// Case: a program enters with the old `?47h` but exits with the
    /// terminfo `?1049l`.
    #[test]
    fn decrst_1049_after_decset_47_restores_the_existing_checkpoint() {
        let mut session = Session::new();
        session.feed(b"\x1b[1;4H\x1b[?47h\x1b[?1049l");
        assert_eq!(session.cursor_column(), 0);
    }

    /// Asserts that a `?47l` between `?1049h` and `?1049l` leaves the
    /// saved cursor unrestored.
    ///
    /// Case: a program enters with `?1049h`, leaves with the bare
    /// `?47l`, and its `rmcup` string sends `?1049l` afterwards.
    #[test]
    fn decrst_47_then_decrst_1049_never_restores_the_saved_cursor() {
        let mut session = Session::new();
        session.feed(b"\x1b[1;2H\x1b[?1049h\x1b[?47l\x1b[1;4H\x1b[?1049l");
        assert_eq!(session.cursor_column(), 3);
    }

    /// Asserts that in `CSI ? 47;1049 h` the 47 enters first and the
    /// 1049 then does nothing: no save, no erase.
    ///
    /// Case: a program lists both alternate-screen numbers in one
    /// DECSET to satisfy old and new terminals at once.
    #[test]
    fn decset_47_and_1049_in_one_sequence_enters_without_saving() {
        let mut session = Session::new();
        session.feed(b"\x1b[?47hx\x1b[?47l\x1b[1;2H\x1b7\x1b[1;4H");
        session.feed(b"\x1b[?47;1049h");
        assert_eq!(session.active_screen(), ScreenKind::Alternate);
        assert_eq!(session.char_at(0, 0), 'x');
        session.feed(b"\x1b[?1049l");
        assert_eq!(session.cursor_column(), 1);
    }

    /// Asserts that in `CSI ? 1049;47 l` the 1049 flips back and
    /// restores, and the 47 then does nothing.
    ///
    /// Case: a program lists both alternate-screen numbers in one
    /// DECRST on the way out.
    #[test]
    fn decrst_1049_and_47_in_one_sequence_restores_once() {
        let mut session = Session::new();
        session.feed(b"\x1b[1;2H\x1b[?1049h\x1b[?47l\x1b[1;4H\x1b[?47h");
        let output = session.feed(b"\x1b[?1049;47l");
        assert_eq!(session.active_screen(), ScreenKind::Primary);
        assert!(output.damaged);
        assert_eq!(session.cursor_column(), 1);
    }
}
