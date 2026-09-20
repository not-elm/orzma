//! Error surface for `orzma_tty`.

use std::io::Error as IoError;

/// Crate-wide result alias defaulting to `()` on success.
pub type OrzmaTtyResult<T = ()> = Result<T, OrzmaTtyError>;

/// Failure while spawning or driving a terminal.
#[derive(Debug, thiserror::Error)]
pub enum OrzmaTtyError {
    /// Opening the PTY pair failed.
    #[error("PTY open failed")]
    PtyOpen(#[source] anyhow::Error),
    /// Spawning the shell child under the PTY slave failed.
    #[error("shell spawn failed")]
    SpawnShell(#[source] anyhow::Error),
    /// Cloning the reader / taking the writer from the PTY master failed.
    #[error("PTY pipe setup failed")]
    PtyPipe(#[source] anyhow::Error),
    /// The writer thread's write to the PTY master failed; the next attempt
    /// to queue a write reports it once, and that write is not queued.
    #[error("PTY write failed")]
    PtyWrite(#[source] IoError),
    /// A write did not fit in the PTY input queue's byte cap and was
    /// dropped whole.
    #[error(
        "write does not fit in the PTY input queue; {dropped_in_episode} dropped since it was last empty"
    )]
    PtyWriteQueueFull {
        /// Writes dropped since the queue was last empty, this one included.
        dropped_in_episode: u64,
    },
    /// The PTY writer no longer accepts writes: its failure was already
    /// reported, or the PTY is closing.
    #[error("PTY writer closed")]
    PtyWriterClosed,
    /// Starting the PTY writer thread failed.
    #[error("PTY writer thread spawn failed")]
    PtyWriterThread(#[source] IoError),
    /// Resizing the PTY master (`TIOCSWINSZ`) failed.
    #[error("PTY resize failed")]
    PtyResize(#[source] anyhow::Error),
    /// The VT interpreted none of a non-empty chunk.
    #[error("the VT interpreted none of a {len}-byte chunk")]
    VtConsumedNothing {
        /// Bytes the chunk still held.
        len: usize,
    },
    /// The VT reported interpreting more bytes than the chunk held.
    #[error("the VT interpreted {consumed} bytes of a {len}-byte chunk")]
    VtConsumedBeyondChunk {
        /// Bytes the VT claimed.
        consumed: usize,
        /// Bytes the chunk held.
        len: usize,
    },
}
