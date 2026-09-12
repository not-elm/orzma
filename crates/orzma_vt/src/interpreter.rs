//! Byte-stream decoding: the vtparse parser and the synchronized-update
//! buffer.

pub(crate) mod apc;

mod csi;
mod osc;
mod sgr;

use crate::device::modes::{AutoWrap, InsertReplaceMode, KeypadMode, ScreenKind, TextCursorEnable};
use crate::interpreter::apc::WebviewApcRequest;
use crate::interpreter::csi::CsiParams;
use crate::interpreter::osc::{
    OscTerminator, PaletteRequest, current_dir, palette_reply, window_title,
};
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

/// The parser plus the synchronized-update buffer.
pub(crate) struct Interpreter {
    parser: VTParser,
    sync: SyncBuffer,
}

impl Interpreter {
    /// Decodes one chunk, applying each action to the borrowed
    /// components and collecting everything the chunk produced into
    /// `output`.
    ///
    /// # Invariants
    ///
    /// The placements the chunk strands are named in its own
    /// [`InterpretOutput::signals`], after every signal the chunk's
    /// actions raised.
    pub fn parse(
        &mut self,
        output: &mut InterpretOutput,
        device: &mut DeviceState,
        tracker: &mut FrameTracker,
        chunk: &[u8],
    ) {
        let cursor_before = device.cursor();
        let mut executor = Executor {
            output,
            sync: &mut self.sync,
            device,
            tracker,
            current_byte: 0,
        };
        for &byte in chunk {
            // NOTE: `current_byte` must be written before `parse_byte`
            // runs, because vtparse calls `osc_dispatch` while it
            // consumes the byte that ends the command. Writing it
            // afterwards would close each reply with the byte before.
            executor.current_byte = byte;
            self.parser.parse_byte(byte, &mut executor);
        }
        executor.sweep_evictions();
        executor.output.damaged |= cursor_before != executor.device.cursor();
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

/// The buffer for a synchronized update (CSI ?2026).
///
/// TODO: hold back the bytes of an open synchronized update.
#[derive(Default)]
struct SyncBuffer {}

/// The temporary view a parser callback applies its action through.
struct Executor<'a> {
    output: &'a mut InterpretOutput,
    #[expect(
        dead_code,
        reason = "the CSI ?2026 synchronized-update buffering will read this seam"
    )]
    sync: &'a mut SyncBuffer,
    device: &'a mut DeviceState,
    tracker: &'a mut FrameTracker,
    /// The byte the parser is consuming. A dispatch callback runs while
    /// the byte that ends its sequence is consumed, so `osc_dispatch`
    /// reads the command's terminator here.
    current_byte: u8,
}

impl VTActor for Executor<'_> {
    fn print(&mut self, b: char) {
        // NOTE: DEL is dropped before `Screen::print` rather than inside
        // it, so that it cannot spend a pending single shift — only a
        // graphic character may do that.
        if b == '\u{7f}' {
            return;
        }
        let modes = self.device.modes();
        let damage =
            self.device
                .active_screen_mut()
                .print(b, modes.insert_replace, modes.auto_wrap);
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
            // HP memory lock / unlock
            // NOTE: Memory lock is ignored rather than honored. It "locks
            // memory above the cursor" (xterm-ctlseqs.pdf p.9), so honoring
            // it would let a stray `ESC l` keep every row above the cursor
            // from scrolling until an `ESC m` that may never come.
            (b'l' | b'm', []) => {}
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
        // NOTE: A truncated sequence must not reach the match. vtparse raises
        // this flag both when it discards an intermediate past its own cap and
        // when a full parameter buffer keeps a trailing intermediate from
        // being promoted; in the second case the lost byte would make
        // `CSI 1;2$r` arrive as the `CSI Pt;Pb r` of DECSTBM and move the
        // scrolling margins.
        if parameters_truncated {
            return;
        }
        match (params.private(), params.intermediates(), byte) {
            // CUP, HVP
            (None, [], b'H' | b'f') => self
                .device
                .active_screen_mut()
                .move_cursor_to(params.value(0), params.value(1)),
            // CHA, HPA
            (None, [], b'G' | b'`') => self
                .device
                .active_screen_mut()
                .move_cursor_to_column(params.value(0)),
            // VPA
            (None, [], b'd') => self
                .device
                .active_screen_mut()
                .move_cursor_to_line(params.value(0)),
            // DECSTBM
            (None, [], b'r') => self
                .device
                .active_screen_mut()
                .set_scroll_region(params.value(0), params.value(1)),
            // SCOSC
            (None, [], b's') if params.values().count() == 0 => {
                self.device.active_screen_mut().save_checkpoint()
            }
            // SCORC
            (None, [], b'u') if params.values().count() == 0 => {
                self.device.active_screen_mut().restore_checkpoint()
            }
            // DA1
            (None, [], b'c') if params.value(0).unwrap_or(0) == 0 => self.reply(PRIMARY_ATTRIBUTES),
            // DSR
            (None, [], b'n') => match params.value(0) {
                Some(5) => self.reply(DEVICE_OK),
                Some(6) => {
                    let (row, column) = self.device.active_screen().cursor_position_report();
                    self.reply(format!("\x1b[{row};{column}R").as_bytes());
                }
                _ => {}
            },
            // NOTE: Title reporting (`CSI 20 t` for the icon label, `CSI
            // 21 t` for the window title) is deliberately not implemented.
            // The real attack surface of terminal titles is the report
            // direction, not the set direction: an attacker sets a title
            // containing a shell command and then asks the terminal to
            // report it back into the shell's input. Several terminals
            // shipped exploitable versions of this (ConEmu's variant was
            // CVE-2022-46387 and CVE-2023-39150).
            // XTWINOPS 22
            (None, [], b't') if params.value(0) == Some(22) => self.device.push_title(),
            // XTWINOPS 23
            (None, [], b't') if params.value(0) == Some(23) => self.pop_title(),
            // MC
            // NOTE: Every media copy is ignored rather than honored, printer
            // controller mode included. That mode sends all later output to
            // the printer "without displaying them on the screen" (vt510.pdf
            // p.323), so honoring it with no printer attached would let a
            // stray `CSI 5 i` hide everything up to a `CSI 4 i` that may
            // never come.
            (None | Some(b'?'), [], b'i') => {}
            // CUU
            (None, [], b'A') => self
                .device
                .active_screen_mut()
                .move_cursor_up(repeat_count(params.value(0))),
            // CUD
            (None, [], b'B') => self
                .device
                .active_screen_mut()
                .move_cursor_down(repeat_count(params.value(0))),
            // CUF, HPR
            (None, [], b'C' | b'a') => self
                .device
                .active_screen_mut()
                .move_cursor_right(repeat_count(params.value(0))),
            // CUB
            (None, [], b'D') => self
                .device
                .active_screen_mut()
                .move_cursor_left(repeat_count(params.value(0))),
            // CNL
            (None, [], b'E') => {
                let screen = self.device.active_screen_mut();
                screen.move_cursor_down(repeat_count(params.value(0)));
                screen.carriage_return();
            }
            // CPL
            (None, [], b'F') => {
                let screen = self.device.active_screen_mut();
                screen.move_cursor_up(repeat_count(params.value(0)));
                screen.carriage_return();
            }
            // ED
            (None, [], b'J') => {
                if let Some(mode) = EraseScreenMode::from_ed(params.value(0).unwrap_or(0)) {
                    self.erase_in_display(mode);
                }
            }
            // EL
            (None, [], b'K') => {
                if let Some(mode) = EraseLineMode::from_el(params.value(0).unwrap_or(0)) {
                    let auto_wrap = self.device.modes().auto_wrap;
                    let damage = self
                        .device
                        .active_screen_mut()
                        .erase_in_line(mode, auto_wrap);
                    self.stage(damage);
                }
            }
            // ECH
            (None, [], b'X') => {
                let auto_wrap = self.device.modes().auto_wrap;
                let damage = self
                    .device
                    .active_screen_mut()
                    .erase_chars(repeat_count(params.value(0)), auto_wrap);
                self.stage(damage);
            }
            // IL
            (None, [], b'L') => {
                let damage = self
                    .device
                    .active_screen_mut()
                    .insert_lines(repeat_count(params.value(0)));
                self.stage(damage);
            }
            // DL
            (None, [], b'M') => {
                let damage = self
                    .device
                    .active_screen_mut()
                    .delete_lines(repeat_count(params.value(0)));
                self.stage(damage);
            }
            // ICH
            (None, [], b'@') => {
                let damage = self
                    .device
                    .active_screen_mut()
                    .insert_characters(repeat_count(params.value(0)));
                self.stage(damage);
            }
            // DCH
            (None, [], b'P') => {
                let damage = self
                    .device
                    .active_screen_mut()
                    .delete_characters(repeat_count(params.value(0)));
                self.stage(damage);
            }
            // SU
            (None, [], b'S') => {
                let damage = self
                    .device
                    .active_screen_mut()
                    .scroll_region_up(repeat_count(params.value(0)));
                self.stage(damage);
            }
            // SD
            // NOTE: xterm's highlight mouse tracking (`CSI Ps;Ps;Ps;Ps;Ps T`,
            // XTHIMOUSE) shares this final byte with no private marker, so
            // only the one-parameter spelling is a scroll down; without the
            // guard a mouse-tracking request would scroll the screen.
            (None, [], b'T') if params.values().count() <= 1 => {
                let damage = self
                    .device
                    .active_screen_mut()
                    .scroll_region_down(repeat_count(params.value(0)));
                self.stage(damage);
            }
            // SD (xterm's alternate spelling)
            (None, [], b'^') => {
                let damage = self
                    .device
                    .active_screen_mut()
                    .scroll_region_down(repeat_count(params.value(0)));
                self.stage(damage);
            }
            // CHT
            (None, [], b'I') => self
                .device
                .active_screen_mut()
                .move_forward_tabs(repeat_count(params.value(0))),
            // CBT
            (None, [], b'Z') => self
                .device
                .active_screen_mut()
                .move_backward_tabs(repeat_count(params.value(0))),
            // TBC
            (None, [], b'g') => {
                if let Some(edit) = CharacterTabEdit::from_tbc(params.value(0).unwrap_or(0)) {
                    self.device.active_screen_mut().edit_tab_stop(edit);
                }
            }
            // CTC
            (None, [], b'W') => {
                if let Some(edit) = CharacterTabEdit::from_ctc(params.value(0).unwrap_or(0)) {
                    self.device.active_screen_mut().edit_tab_stop(edit);
                }
            }
            // DECST8C
            (Some(b'?'), [], b'W') if params.value(0) == Some(5) => {
                self.device.active_screen_mut().reset_tab_stops()
            }
            // SGR
            (None, [], b'm') => {
                let pen = self.device.active_screen_mut().pen_mut();
                *pen = pen.applied(&params);
            }
            // SM
            (None, [], b'h') => self.set_modes(&params, true),
            // RM
            (None, [], b'l') => self.set_modes(&params, false),
            // DECSET
            (Some(b'?'), [], b'h') => self.set_private_modes(&params, true),
            // DECRST
            (Some(b'?'), [], b'l') => self.set_private_modes(&params, false),
            // DA2
            (Some(b'>'), [], b'c') if params.value(0).unwrap_or(0) == 0 => {
                self.reply(&secondary_attributes())
            }
            _ => {}
        }
    }

    // TODO: Implement the remaining OSC handlers — the dynamic colors
    // (OSC 10 / 11 / 12), hyperlinks (OSC 8), and the clipboard (OSC 52).
    fn osc_dispatch(&mut self, params: &[&[u8]]) {
        if let Some(title) = window_title(params) {
            self.device.set_title(Some(title.clone()));
            self.signal(VtSignal::Title(title));
        }
        if let Some(path) = current_dir(params) {
            self.signal(VtSignal::CurrentDir(path));
        }
        self.apply_palette_requests(params);
    }

    fn apc_dispatch(&mut self, data: Vec<u8>) {
        let Some(request) = WebviewApcRequest::parse(&data) else {
            return;
        };
        // NOTE: An accepted mount and a hit unmount must raise the chunk
        // liveness themselves. `signal` deliberately does not, so dropping
        // these assignments would leave the changed placement list without
        // a frame to carry it — the webview would register and never draw.
        let signal = match request {
            WebviewApcRequest::Mount { instance, size } => {
                if self.device.mount_placement(size, instance) {
                    self.output.damaged = true;
                    VtSignal::WebviewMount { instance, size }
                } else {
                    VtSignal::WebviewMountRejected { instance }
                }
            }
            WebviewApcRequest::Unmount { instance } => {
                self.output.damaged |= self.device.unmount_placement(instance);
                VtSignal::WebviewUnmount { instance }
            }
        };
        self.signal(signal);
    }
}

/// The control-function steps the parser callbacks delegate to, and the
/// helpers that record what a chunk produced.
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

    /// Erases part of the active screen with its pen background (ED,
    /// and the alternate-screen modes that blank the screen they show
    /// or leave).
    fn erase_in_display(&mut self, mode: EraseScreenMode) {
        let damage = self.device.active_screen_mut().erase_in_display(mode);
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
    /// A reply leaves the chunk liveness alone.
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

    /// Applies the palette requests an `OSC 4` or `OSC 104` carries, in
    /// order, answering each query with the slot's colour at that
    /// point.
    ///
    /// A command that changes a slot stages one full repaint, whatever
    /// the number of requests it carries.
    fn apply_palette_requests(&mut self, params: &[&[u8]]) {
        let terminator = OscTerminator::from_byte(self.current_byte);
        let mut changed = false;
        for request in PaletteRequest::parse(params) {
            match request {
                PaletteRequest::Set { index, color } => {
                    changed |= self.device.set_indexed_color(index, color);
                }
                PaletteRequest::Reset { index } => {
                    changed |= self.device.reset_indexed_color(index);
                }
                PaletteRequest::ResetAll => changed |= self.device.reset_indexed_colors(),
                PaletteRequest::Query { index } => {
                    let color = self.device.palette().indexed[usize::from(index)];
                    self.reply(&palette_reply(index, color, terminator));
                }
            }
        }
        if changed {
            self.stage(Some(DamageSpan::Full));
        }
    }

    /// Returns every screen and mode to its power-up state (RIS),
    /// reporting the title's return to the host's default.
    ///
    /// A reset that finds no title reports nothing.
    fn reset_device(&mut self) {
        let had_title = self.device.title().is_some();
        let damage = self.device.reset();
        self.stage(damage);
        if had_title {
            self.signal(VtSignal::ResetTitle);
        }
    }

    /// Names the placements this chunk stranded and raises the chunk
    /// liveness.
    ///
    /// A chunk strands a placement when its anchor row leaves the grid:
    /// a reset drops every row, and a scroll drops a row when it recycles
    /// the row rather than keeping it in history — past the cap, inside a
    /// scroll region, downward at the top margin, or on a screen without
    /// scrollback.
    ///
    /// It must run once per chunk, after the chunk's last action, so
    /// that a placement the chunk strands and then re-mounts is updated
    /// in place rather than evicted and re-created.
    fn sweep_evictions(&mut self) {
        let Some(evicted) = VtSignal::evicted(self.device.evict_lost_anchors()) else {
            return;
        };
        self.signal(evicted);
        self.output.damaged = true;
    }
}

/// The control functions a CSI sequence requests, where one final byte
/// stands for a list of independent settings.
impl Executor<'_> {
    /// Applies every ANSI mode this terminal implements out of one `SM`
    /// or `RM` sequence, ignoring the numbers it does not.
    fn set_modes(&mut self, params: &CsiParams<'_>, enabled: bool) {
        for mode in params.values().flatten() {
            // IRM
            if mode == 4 {
                self.device.modes_mut().insert_replace = InsertReplaceMode::from_sm(enabled);
            }
        }
    }

    /// Applies every private mode this terminal implements out of one
    /// `DECSET` or `DECRST` sequence, ignoring the numbers it does not.
    fn set_private_modes(&mut self, params: &CsiParams<'_>, enabled: bool) {
        for mode in params.values().flatten() {
            match mode {
                // DECCKM
                1 => self.device.modes_mut().app_cursor = enabled,
                // DECCOLM
                // NOTE: The column mode is ignored rather than honored, the
                // side effects included. A pane's width is not the VT's to
                // set — the size flows one way, from the window geometry
                // through the multiplexer layout to the PTY — so the erase
                // and the margin reset vt510.pdf p.143 gives DECCOLM would
                // destroy the page for a width change that never follows,
                // and `xterm-256color` puts `CSI ? 3 l` in the `is2` string
                // every `tput init` sends. xterm, whose entry that is, gates
                // the whole sequence behind a resource that is off by
                // default and so reaches the same no-op.
                3 => {}
                // DECOM
                6 => self
                    .device
                    .active_screen_mut()
                    .set_origin_mode(OriginMode::from_decset(enabled)),
                // DECAWM
                7 => self.device.set_auto_wrap(AutoWrap::from_decset(enabled)),
                // DECTCEM
                25 => {
                    self.device.modes_mut().text_cursor_enable =
                        TextCursorEnable::from_decset(enabled);
                }
                // Alternate screen
                47 => self.switch_screen(ScreenKind::from_decset(enabled)),
                // DECNKM
                66 => self.device.modes_mut().keypad_mode = KeypadMode::from_decset(enabled),
                // XTFOCUS
                1004 => self.device.modes_mut().focus_in_out = enabled,
                // Alternate scroll
                1007 => self.device.modes_mut().alternate_scroll = enabled,
                // Interpret "meta" key
                // NOTE: The eighth-bit meta encoding is ignored rather than
                // honored. Alt always prefixes ESC, which is xterm's
                // metaSendsEscape, and xterm gives that precedence over this
                // mode; honoring it would let the `smm` readline sends at
                // startup switch every Alt key away from the ESC prefix.
                1034 => {}
                // Alternate screen, erased on exit
                1047 => self.set_alternate_screen_erased_on_exit(enabled),
                // DECSC / DECRC
                1048 if enabled => self.device.active_screen_mut().save_checkpoint(),
                1048 => self.device.active_screen_mut().restore_checkpoint(),
                // Alternate screen with DECSC / DECRC
                1049 => self.set_alternate_screen_with_cursor(enabled),
                // Bracketed paste
                2004 => self.device.modes_mut().bracketed_paste = enabled,
                _ => self.set_mouse_mode(mode, enabled),
            }
        }
    }

    /// Applies a mouse tracking level or report encoding; a number
    /// neither answers is ignored.
    fn set_mouse_mode(&mut self, mode: u16, enabled: bool) {
        let modes = self.device.modes_mut();
        if let Some(tracking) = modes.mouse_tracking.with_decset(mode, enabled) {
            modes.mouse_tracking = tracking;
        } else if let Some(encoding) = modes.mouse_encoding.with_decset(mode, enabled) {
            modes.mouse_encoding = encoding;
        }
    }

    /// Applies `DECSET 1049` / `DECRST 1049`: a set saves the primary
    /// screen's cursor, shows the alternate screen, and erases it; a
    /// reset shows the primary screen and restores that cursor.
    ///
    /// Each direction runs only from the other screen, so a set already
    /// on the alternate screen cannot overwrite that screen's own DECSC
    /// slot, and a stray reset on the primary screen leaves the cursor
    /// alone. The erase fills with the pen the alternate screen kept
    /// from its previous use rather than the primary screen's.
    fn set_alternate_screen_with_cursor(&mut self, enabled: bool) {
        match (enabled, self.device.modes().active_screen) {
            (true, ScreenKind::Primary) => {
                self.device.active_screen_mut().save_checkpoint();
                self.switch_screen(ScreenKind::Alternate);
                self.erase_in_display(EraseScreenMode::All);
            }
            (false, ScreenKind::Alternate) => {
                self.switch_screen(ScreenKind::Primary);
                self.device.active_screen_mut().restore_checkpoint();
            }
            (true, ScreenKind::Alternate) | (false, ScreenKind::Primary) => {}
        }
    }

    /// Applies `DECSET 1047` / `DECRST 1047`: a set is a bare flip; a
    /// reset erases the alternate screen, when it is the one shown, and
    /// then flips back.
    fn set_alternate_screen_erased_on_exit(&mut self, enabled: bool) {
        if !enabled && self.device.modes().active_screen == ScreenKind::Alternate {
            self.erase_in_display(EraseScreenMode::All);
        }
        self.switch_screen(ScreenKind::from_decset(enabled));
    }

    /// Shows `to`, naming the placements a return to the primary screen
    /// tears down. Already showing `to` is a no-op that stages nothing
    /// and signals nothing.
    ///
    /// # Invariants
    ///
    /// Every flip stages [`DamageSpan::Full`], so the frame after it
    /// carries every viewport row.
    fn switch_screen(&mut self, to: ScreenKind) {
        if self.device.modes().active_screen == to {
            return;
        }
        let placements = self.device.switch_screen(to);
        if let Some(evicted) = VtSignal::evicted(placements) {
            self.signal(evicted);
        }
        self.stage(Some(DamageSpan::Full));
    }
}

/// A repeat count parameter, where an omitted or zero value means one.
///
/// VT220 states "a parameter of 0 or 1" for these counts.
fn repeat_count(value: Option<u16>) -> u16 {
    match value {
        None | Some(0) => 1,
        Some(count) => count,
    }
}

/// The DA1 response: a VT102 with no extensions. This terminal does not
/// implement Sixel, DRCS, or selective erase.
const PRIMARY_ATTRIBUTES: &[u8] = b"\x1b[?6c";

/// The DSR 5 response: the terminal is operating normally.
const DEVICE_OK: &[u8] = b"\x1b[0n";

/// Builds the DA2 response.
///
/// The terminal type is 0 ("VT100"), and the cartridge number is 1
/// rather than the zero xterm documents.
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
/// Every component must stay below one hundred.
fn pack_version(major: u32, minor: u32, patch: u32) -> u32 {
    major * 10_000 + minor * 100 + patch
}

#[cfg(test)]
mod tests;
