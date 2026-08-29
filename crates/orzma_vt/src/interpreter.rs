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

use crate::device::modes::KeypadMode;
use crate::interpreter::csi::CsiParams;
use crate::screen::character_sets::{CharacterSet, GCode, SingleShift};
use crate::screen::margins::OriginMode;
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
            (b'c', []) => {
                let damage = self.device.reset();
                self.stage(damage);
            }
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

    // TODO: Implement the OSC handlers — the title stack (OSC 0 / 1 /
    // 2), the palette (OSC 4 / 10 / 11 / 12), the working directory
    // (OSC 7), hyperlinks (OSC 8), and the clipboard (OSC 52).
    fn osc_dispatch(&mut self, _params: &[&[u8]]) {}

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
                // DECOM
                6 => self
                    .device
                    .active_screen_mut()
                    .set_origin_mode(OriginMode::from_decset(enabled)),
                // DECNKM
                66 => self.device.modes_mut().keypad_mode = KeypadMode::from_decset(enabled),
                _ => {}
            }
        }
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
    use crate::screen::cell::Cell;
    use crate::screen::grid::GridSize;
    use crate::screen::grid::coords::GridColumn;
    use crate::screen::viewport::ViewportLine;
    use crate::{OrzmaVt, Vt};

    /// Runs `chunk` through a fresh interpreter and hands back the
    /// device it wrote to together with everything the chunk produced.
    ///
    /// The interpreter is driven directly rather than through
    /// [`OrzmaVt`] because these tests read [`DeviceState`], which the
    /// [`Vt`] trait does not expose.
    fn interpret_fully(chunk: &[u8]) -> (DeviceState, InterpretOutput) {
        let mut device = DeviceState::new(GridSize { cols: 4, rows: 3 }, 10);
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

    /// Runs `setup` and then `chunk` over one interpreter and device,
    /// and reports the liveness `chunk` alone produced.
    fn liveness_after(setup: &[u8], chunk: &[u8]) -> bool {
        let mut device = DeviceState::new(GridSize { cols: 4, rows: 3 }, 10);
        let mut tracker = FrameTracker::new();
        let mut interpreter = Interpreter::default();
        let mut damaged = false;
        for bytes in [setup, chunk] {
            let mut output = InterpretOutput::default();
            interpreter.parse(&mut output, &mut device, &mut tracker, bytes);
            damaged = output.damaged;
        }
        damaged
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
    /// Case: a shell sets a colour with `CSI 0 m` on a terminal that has
    /// no SGR yet.
    #[test]
    fn an_unimplemented_sequence_is_ignored() {
        let device = interpret(b"\x1b[0ma");
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
}
