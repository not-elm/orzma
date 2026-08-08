//! Error surface for `orzma_term`.

/// Crate-wide result alias defaulting to `()` on success.
pub type OrzmaTermResult<T = ()> = Result<T, OrzmaTermError>;

/// Failure while spawning or driving a terminal.
#[derive(Debug, thiserror::Error)]
pub enum OrzmaTermError {
    /// Opening the PTY pair failed.
    #[error("PTY open failed")]
    PtyOpen(#[source] anyhow::Error),
    /// Spawning the shell child under the PTY slave failed.
    #[error("shell spawn failed")]
    SpawnShell(#[source] anyhow::Error),
    /// Cloning the reader / taking the writer from the PTY master failed.
    #[error("PTY pipe setup failed")]
    PtyPipe(#[source] anyhow::Error),
    /// Write a pty input buffer to the PTY master failed.
    #[error("PTY write failed")]
    PtyWrite(#[source] std::io::Error),
}
