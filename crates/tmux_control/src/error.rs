//! Error and result types for the tmux control-mode client.

use thiserror::Error;

/// Result alias for the tmux control client.
pub type TmuxResult<T = ()> = Result<T, TmuxError>;

/// An error produced by the tmux control-mode client.
#[derive(Error, Debug)]
pub enum TmuxError {
    /// Transport I/O failed.
    #[error("transport I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// The underlying parser rejected a line.
    #[error("parser error: {0}")]
    Parse(#[from] tmux_control_parser::TmuxError),
    /// A reply block arrived with no pending command to correlate it to.
    #[error("unsolicited reply (number {number}) with no pending command")]
    UnsolicitedReply {
        /// The tmux command number carried by the orphan reply.
        number: u32,
    },
    /// A command string contained an embedded newline.
    #[error("command contains an embedded newline")]
    InvalidCommand,
    /// A `list-sessions` output line could not be parsed.
    #[error("malformed list-sessions line: {line}")]
    MalformedSessionList {
        /// The offending line, verbatim.
        line: String,
    },
    /// A `list-windows` output line could not be parsed.
    #[error("malformed list-windows line: {line}")]
    MalformedWindowList {
        /// The offending line, verbatim.
        line: String,
    },
    /// Spawning the tmux process failed.
    // NOTE: `Io` carries `#[from]`, so `?` on an io::Error yields `Io`, not
    // `Spawn`. Construct `Spawn(e)` explicitly via `map_err` at spawn sites.
    #[error("failed to spawn tmux")]
    Spawn(std::io::Error),
}
