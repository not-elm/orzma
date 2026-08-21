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

use std::sync::mpsc::Sender;

use crate::{
    damage::DamageLedger, device::DeviceState, placement::PlacementStore, schema::VtSignal,
};
use vtparse::{VTActor, VTParser};

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
        self.device.active_mut().print(b);
    }

    fn execute_c0_or_c1(&mut self, control: u8) {
        match control {
            0x07 => {
                let _ = self.signal_tx.send(VtSignal::Bell);
            }
            0x0A => {}
            _ => {}
        }
    }

    fn dcs_hook(
        &mut self,
        mode: u8,
        params: &[i64],
        intermediates: &[u8],
        ignored_excess_intermediates: bool,
    ) {
        todo!()
    }

    fn dcs_put(&mut self, byte: u8) {
        todo!()
    }

    fn dcs_unhook(&mut self) {
        todo!()
    }

    fn esc_dispatch(
        &mut self,
        params: &[i64],
        intermediates: &[u8],
        ignored_excess_intermediates: bool,
        byte: u8,
    ) {
        todo!()
    }

    fn csi_dispatch(&mut self, params: &[vtparse::CsiParam], parameters_truncated: bool, byte: u8) {
        todo!()
    }

    fn osc_dispatch(&mut self, params: &[&[u8]]) {
        todo!()
    }

    fn apc_dispatch(&mut self, data: Vec<u8>) {
        todo!()
    }
}
