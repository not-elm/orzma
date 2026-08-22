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
                let damage = self.device.active_mut().bs();
                self.damage.stage_if_changed(damage);
            }
            0x09 => {
                let damage = self.device.active_mut().ht();
                self.damage.stage_if_changed(damage);
            }
            0x0A | 0x0B | 0x0C | 0x84 => {
                let damage = self.device.active_mut().lf();
                self.damage.stage_if_changed(damage);
            }
            0x0D => {
                let damage = self.device.active_mut().cr();
                self.damage.stage_if_changed(damage);
            }
            0x85 => {
                self.damage.stage_if_changed(self.device.active_mut().cr());
                self.damage.stage_if_changed(self.device.active_mut().lf());
            }
            0x88 => self.device.active_mut().hts(),
            0x8D => {
                let damage = self.device.active_mut().ri();
                self.damage.stage_if_changed(damage);
            }
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
        _intermediates: &[u8],
        _ignored_excess_intermediates: bool,
        _byte: u8,
    ) {
        todo!()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{GridSize, ViewportLine};
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
}
