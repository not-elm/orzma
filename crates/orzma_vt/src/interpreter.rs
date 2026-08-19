//! Byte-stream decoding: the vtparse parser and the synchronized-update
//! buffer.
//!
//! [`Interpreter`] turns PTY bytes into parser actions. It changes no
//! device state itself — the executor it dispatches to does that —
//! and it owns the CSI ?2026 buffer, so a synchronized update holds its
//! bytes here until the application closes it.
#![expect(
    dead_code,
    reason = "OrzmaVt::interpret reaches the parser once the executor lands"
)]

use vtparse::{VTActor, VTParser};

/// The parser plus the bytes a synchronized update is holding back.
pub(crate) struct Interpreter {
    parser: VTParser,
    sync: SyncBuffer,
}

impl Interpreter {
    /// Builds an interpreter with an idle parser and an empty
    /// synchronized-update buffer.
    pub fn new() -> Self {
        todo!()
    }

    /// Decodes one chunk, dispatching each action to `actor`.
    pub fn parse(&mut self, _actor: &mut dyn VTActor, _chunk: &[u8]) {
        todo!()
    }
}

/// Bytes held back while a synchronized update (CSI ?2026) is open.
// TODO: Carry the buffered bytes plus the nesting depth, and flush them
// before an APC mount samples the cursor.
struct SyncBuffer {}
