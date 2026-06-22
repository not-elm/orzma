//! Error and result types for the tmux control-mode client.

use thiserror::Error;

/// Result alias for the tmux control client.
pub type TmuxResult<T = ()> = Result<T, TmuxError>;

/// An error produced by the tmux control-mode client.
#[derive(Error, Debug)]
pub enum TmuxError {
    /// The underlying parser rejected a line.
    #[error("parser error: {0}")]
    Parse(#[from] tmux_control_parser::TmuxError),
    /// A command string contained an embedded newline.
    #[error("command contains an embedded newline")]
    InvalidCommand,
}
