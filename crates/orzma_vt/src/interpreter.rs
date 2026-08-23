//! Byte-stream decoding: the vtparse parser and the synchronized-update
//! buffer.
//!
//! [`Interpreter`] turns PTY bytes into parser actions. It changes no
//! device state itself — the executor it dispatches to does that —
//! and it owns the CSI ?2026 buffer, so a synchronized update holds its
//! bytes here until the application closes it.
#![expect(
    dead_code,
    reason = "OrzmaVt::interpret reaches the parser once the executor's callbacks land"
)]

use crate::{
    damage::DamageLedger, device::DeviceState, placement::PlacementStore, schema::VtSignal,
};
use std::sync::mpsc::Sender;
use vtparse::{CsiParam, VTActor, VTParser};

/// The parser plus the bytes a synchronized update is holding back.
pub(crate) struct Interpreter {
    parser: VTParser,
    sync: SyncBuffer,
}

impl Interpreter {
    /// Decodes one chunk, applying each action to the borrowed
    /// components through an [`Executor`].
    ///
    /// The executor is built here rather than passed in because it
    /// borrows [`SyncBuffer`], which `&mut self` already holds.
    pub fn parse(
        &mut self,
        device: &mut DeviceState,
        placements: &mut PlacementStore,
        damage: &mut DamageLedger,
        signal_tx: &mut Sender<VtSignal>,
        chunk: &[u8],
    ) {
        let mut executor = Executor {
            sync: &mut self.sync,
            device,
            placements,
            damage,
            signal_tx,
        };
        self.parser.parse(chunk, &mut executor);
        todo!()
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
// TODO: Carry the buffered bytes plus the nesting depth, and flush them
// before an APC mount samples the cursor.
#[derive(Default)]
struct SyncBuffer {}

/// The temporary view a parser callback applies its action through.
///
/// Every field is a borrow split from a component [`crate::OrzmaVt`]
/// owns, so the view lives exactly as long as one [`Interpreter::parse`]
/// call and carries no state between chunks.
// TODO: Carry the call-local outbox the signals and replies collect
// into, and implement `VTActor` — the callbacks land with it.
struct Executor<'a> {
    sync: &'a mut SyncBuffer,
    device: &'a mut DeviceState,
    placements: &'a mut PlacementStore,
    damage: &'a mut DamageLedger,
    signal_tx: &'a mut Sender<VtSignal>,
}

impl VTActor for Executor<'_> {
    fn print(&mut self, b: char) {
        let damage = self.device.active_mut().print(b);
        self.damage.stage_if_changed(damage);
    }

    fn execute_c0_or_c1(&mut self, control: u8) {
        match control {
            0x07 => {
                let _ = self.signal_tx.send(VtSignal::Bell);
            }
            0x08 => {
                let damage = self.device.active_mut().backspace();
                self.damage.stage_if_changed(damage);
            }
            0x09 => {
                let damage = self.device.active_mut().move_forward_tabs(1);
                self.damage.stage_if_changed(damage);
            }
            0x0A | 0x0B | 0x0C | 0x84 => self.index(),
            0x0D => {
                let damage = self.device.active_mut().carriage_return();
                self.damage.stage_if_changed(damage);
            }
            0x85 => self.next_line(),
            0x88 => self.device.active_mut().set_horizontal_tab_stop(),
            0x8D => self.reverse_index(),
            _ => {}
        }
    }

    fn dcs_hook(
        &mut self,
        _mode: u8,
        _params: &[i64],
        _intermediates: &[u8],
        _ignored_excess_intermediates: bool,
    ) {
        todo!()
    }

    fn dcs_put(&mut self, _byte: u8) {
        todo!()
    }

    fn dcs_unhook(&mut self) {
        todo!()
    }

    fn esc_dispatch(
        &mut self,
        _params: &[i64],
        intermediates: &[u8],
        _ignored_excess_intermediates: bool,
        byte: u8,
    ) {
        match (byte, intermediates) {
            (b'D', []) => self.index(),
            (b'E', []) => self.next_line(),
            (b'H', []) => self.device.active_mut().set_horizontal_tab_stop(),
            (b'M', []) => self.reverse_index(),
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, _params: &[CsiParam], _parameters_truncated: bool, _byte: u8) {
        todo!()
    }

    fn osc_dispatch(&mut self, _params: &[&[u8]]) {
        todo!()
    }

    fn apc_dispatch(&mut self, _data: Vec<u8>) {
        todo!()
    }
}

/// The control functions an eight-bit C1 byte and its seven-bit `ESC`
/// form both request, so the two spellings cannot drift apart.
impl Executor<'_> {
    /// Moves the cursor down a row, scrolling at the bottom margin (IND,
    /// and the LF family that shares its effect).
    fn index(&mut self) {
        let damage = self.device.active_mut().line_feed();
        self.damage.stage_if_changed(damage);
    }

    /// Returns the carriage and moves the cursor down a row (NEL).
    fn next_line(&mut self) {
        self.damage
            .stage_if_changed(self.device.active_mut().carriage_return());
        self.damage
            .stage_if_changed(self.device.active_mut().line_feed());
    }

    /// Moves the cursor up a row, scrolling at the top margin (RI).
    fn reverse_index(&mut self) {
        let damage = self.device.active_mut().reverse_index();
        self.damage.stage_if_changed(damage);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{GridColumn, GridSize, ViewportLine};
    use std::sync::mpsc::channel;

    /// Runs `chunk` through a parser wired to a fresh executor and hands
    /// back the device it wrote to.
    ///
    /// `Interpreter::parse` is still `todo!()`, so the executor is built
    /// here and driven directly rather than through the public entry
    /// point.
    fn interpret(chunk: &[u8]) -> DeviceState {
        let mut device = DeviceState::new(GridSize { cols: 4, rows: 3 }, 10);
        let mut placements = PlacementStore::new();
        let mut damage = DamageLedger::new();
        let (mut signal_tx, _signal_rx) = channel();
        let mut sync = SyncBuffer::default();
        let mut executor = Executor {
            sync: &mut sync,
            device: &mut device,
            placements: &mut placements,
            damage: &mut damage,
            signal_tx: &mut signal_tx,
        };
        VTParser::new().parse(chunk, &mut executor);
        device
    }

    /// Asserts that the raw C1 byte for RI reaches the screen.
    ///
    /// Case: a program emits an eight-bit reverse index on a terminal not
    /// running in UTF-8 mode.
    #[test]
    fn the_raw_c1_byte_reverse_indexes() {
        let device = interpret(b"a\r\x8d");
        assert_eq!(device.active().viewport_row(ViewportLine(1))[0].c, 'a');
    }

    /// Asserts that the UTF-8 encoding of U+008D reaches the same arm.
    ///
    /// Case: a program running on a UTF-8 stream emits the reverse index.
    #[test]
    fn the_utf8_form_reverse_indexes() {
        let device = interpret(b"a\r\xc2\x8d");
        assert_eq!(device.active().viewport_row(ViewportLine(1))[0].c, 'a');
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
        assert_eq!(device.active().viewport_row(ViewportLine(0))[0].c, 'a');
        assert_eq!(device.active().viewport_row(ViewportLine(1))[1].c, 'b');
    }

    /// Asserts that `ESC E` moves the cursor down a row and returns the
    /// carriage.
    ///
    /// Case: a program ends a log line with the seven-bit next line
    /// instead of writing a CR and an LF of its own.
    #[test]
    fn the_seven_bit_next_line_returns_the_carriage() {
        let device = interpret(b"a\x1bEb");
        assert_eq!(device.active().viewport_row(ViewportLine(0))[0].c, 'a');
        assert_eq!(device.active().viewport_row(ViewportLine(1))[0].c, 'b');
    }

    /// Asserts that `ESC H` plants a tabulation stop at the cursor
    /// column.
    ///
    /// Case: a program sizes a column by walking the cursor to the width
    /// it wants, setting a stop there, and tabbing to it on later rows.
    #[test]
    fn the_seven_bit_tab_set_plants_a_stop_at_the_cursor() {
        let device = interpret(b"ab\x1bH\r\t");
        assert_eq!(device.active().cursor_column(), GridColumn(2));
    }

    /// Asserts that `ESC M` scrolls the region down when the cursor
    /// already sits on the top margin.
    ///
    /// Case: a pager walks backwards through a document with the
    /// seven-bit reverse index while the cursor rests on the first row.
    #[test]
    fn the_seven_bit_reverse_index_scrolls_at_the_top_margin() {
        let device = interpret(b"a\r\x1bM");
        assert_eq!(device.active().viewport_row(ViewportLine(1))[0].c, 'a');
    }
}
