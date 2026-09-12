//! Error surface for `orzma_tty`.

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
    /// Writing an input buffer to the PTY master failed.
    #[error("PTY write failed")]
    PtyWrite(#[source] std::io::Error),
    /// Resizing the PTY master (`TIOCSWINSZ`) failed.
    #[error("PTY resize failed")]
    PtyResize(#[source] anyhow::Error),
}
