//! Error surface for `orzma_tty`.

use std::io::Error as IoError;

/// Crate-wide result alias defaulting to `()` on success.
pub type OrzmaTtyResult<T = ()> = Result<T, OrzmaTtyError>;

/// Failure while spawning or driving a terminal.
#[derive(Debug, thiserror::Error)]
pub enum OrzmaTtyError {
    /// Opening the PTY pair failed.
    #[error("PTY open failed: {0}")]
    PtyOpen(#[source] anyhow::Error),
    /// Spawning the shell child under the PTY slave failed.
    #[error("shell spawn failed: {0}")]
    SpawnShell(#[source] anyhow::Error),
    /// Cloning the reader / taking the writer from the PTY master failed.
    #[error("PTY pipe setup failed: {0}")]
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
    #[error("PTY resize failed: {0}")]
    PtyResize(#[source] anyhow::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts that a PTY open failure's message includes the underlying
    /// cause.
    ///
    /// Case: opening the PTY pair fails on the OS side.
    #[test]
    fn a_pty_open_failure_keeps_the_cause_text() {
        let error = OrzmaTtyError::PtyOpen(anyhow::anyhow!("out of PTYs"));
        assert_eq!(error.to_string(), "PTY open failed: out of PTYs");
    }

    /// Asserts that a shell spawn failure's message includes the
    /// underlying cause.
    ///
    /// Case: the shell fails to spawn under the PTY slave, for example
    /// because the configured shell executable does not exist.
    #[test]
    fn a_spawn_shell_failure_keeps_the_cause_text() {
        let error = OrzmaTtyError::SpawnShell(anyhow::anyhow!("shell.exe not found"));
        assert_eq!(error.to_string(), "shell spawn failed: shell.exe not found");
    }

    /// Asserts that a PTY pipe setup failure's message includes the
    /// underlying cause.
    ///
    /// Case: cloning the reader or taking the writer from the PTY master
    /// fails.
    #[test]
    fn a_pty_pipe_failure_keeps_the_cause_text() {
        let error = OrzmaTtyError::PtyPipe(anyhow::anyhow!("handle already taken"));
        assert_eq!(
            error.to_string(),
            "PTY pipe setup failed: handle already taken"
        );
    }

    /// Asserts that a PTY resize failure's message includes the
    /// underlying cause.
    ///
    /// Case: `TIOCSWINSZ` fails against the PTY master.
    #[test]
    fn a_pty_resize_failure_keeps_the_cause_text() {
        let error = OrzmaTtyError::PtyResize(anyhow::anyhow!("invalid file descriptor"));
        assert_eq!(
            error.to_string(),
            "PTY resize failed: invalid file descriptor"
        );
    }
}
