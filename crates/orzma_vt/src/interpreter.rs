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

use crate::{damage::DamageLedger, device::DeviceState, placement::PlacementStore};
use vtparse::VTParser;

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
        _device: &mut DeviceState,
        _placements: &mut PlacementStore,
        _damage: &mut DamageLedger,
        _chunk: &[u8],
    ) {
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
}
