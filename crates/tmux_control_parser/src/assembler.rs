//! Stateful assembler that groups `%begin`..`%end`/`%error` blocks into replies
//! and passes standalone notifications through.

use crate::error::TmuxResult;
use crate::event::ControlEvent;

/// A higher-level frame emitted by [`BlockAssembler`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    /// A completed command output block.
    Reply {
        /// The command number shared by the matching `%begin`/`%end`/`%error`.
        number: u32,
        /// `true` if closed by `%end`, `false` if closed by `%error`.
        ok: bool,
        /// Reply body lines, verbatim, in order.
        body: Vec<String>,
    },
    /// A standalone notification that occurred outside any block.
    Notification(ControlEvent),
}

/// Groups raw control-mode lines into [`Frame`]s, tracking the active block.
#[derive(Debug, Default, Clone)]
pub struct BlockAssembler;

impl BlockAssembler {
    /// Returns a new assembler with no block open.
    pub fn new() -> Self {
        Self
    }

    /// Feeds one raw line, returning a completed [`Frame`] when one is ready.
    pub fn feed(&mut self, line: &[u8]) -> TmuxResult<Option<Frame>> {
        let _ = line;
        todo!("BlockAssembler::feed")
    }
}
