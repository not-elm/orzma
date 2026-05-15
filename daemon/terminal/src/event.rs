//! Raw byte/exit events broadcast from the PTY reader to subscribers.

use serde::{Deserialize, Serialize};

/// Asynchronous event surfaced from the PTY reader: a raw output chunk or
/// the child's exit status.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum TerminalEvent {
    /// Bytes read from the PTY master.
    Data {
        /// Raw byte chunk as read from the PTY.
        buffer: Vec<u8>,
    },
    /// Child exited; `code` is `None` when the wait failed.
    Exit {
        /// Exit code reported by the child, or `None` if the wait failed.
        code: Option<i32>,
    },
}
