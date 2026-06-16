//! Error types for the ratatui-ozma SDK.

use std::sync::PoisonError;

/// A `Result` whose error is [`OzmaError`].
pub type OzmaResult<T> = Result<T, OzmaError>;

/// An error from the ratatui-ozma SDK.
#[derive(Debug, thiserror::Error)]
pub enum OzmaError {
    /// `$OZMA_SOCK`, or both `$OZMA_TOKEN` and `$TMUX_PANE`, were unset — not
    /// running inside an ozmux pane.
    #[error("not inside an ozmux pane: {0} is unset")]
    NotInPane(&'static str),

    /// A socket connect/read/write failure.
    #[error("control-socket io error: {0}")]
    Io(#[from] std::io::Error),

    /// The resolved control-socket path could not be reached — the file is gone
    /// (`NotFound`) or nothing is listening on it (`ConnectionRefused`). This
    /// means `$OZMA_SOCK` is stale: it points at a control socket whose ozmux has
    /// exited. Distinct from [`OzmaError::Io`] so the caller can tell the user to
    /// re-attach ozmux rather than print the misleading "not in a pane" hint.
    #[error(
        "control socket {path} is unavailable ({cause}); no ozmux is attached to this tmux session — attach ozmux and retry"
    )]
    SocketUnavailable {
        /// The resolved socket path that could not be reached.
        path: String,
        /// The underlying connect error (`NotFound` / `ConnectionRefused`). Named
        /// `cause`, not `source`: the message already renders it inline, and a
        /// field literally named `source` would make `thiserror` also expose it
        /// via `Error::source()`, double-printing it for chain-aware consumers.
        cause: std::io::Error,
    },

    /// The control plane rejected a `register` request.
    #[error("register rejected: {reason}")]
    Register {
        /// The control-plane error string (e.g. `html_too_large`, `unsafe_entry`).
        reason: String,
    },

    /// The connection closed while a `register` reply was pending.
    #[error("control socket closed before register reply")]
    Disconnected,

    /// A serde (de)serialization failure.
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    /// An internal lock was poisoned by a panicked thread.
    #[error("internal lock poisoned")]
    Poisoned,
}

impl<T> From<PoisonError<T>> for OzmaError {
    fn from(_: PoisonError<T>) -> Self {
        OzmaError::Poisoned
    }
}

/// An error returned by an RPC handler, surfaced to the page as a rejected Promise.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct RpcError {
    message: String,
}

impl RpcError {
    /// Creates an `RpcError` with the given message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// Returns the error message sent back to the page.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl From<serde_json::Error> for RpcError {
    fn from(e: serde_json::Error) -> Self {
        RpcError::new(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_rejection_renders_reason() {
        let e = OzmaError::Register {
            reason: "html_too_large".to_owned(),
        };
        assert!(e.to_string().contains("html_too_large"));
    }

    #[test]
    fn rpc_error_message_roundtrips() {
        let e = RpcError::new("unknown_method");
        assert_eq!(e.message(), "unknown_method");
    }

    #[test]
    fn io_error_converts() {
        let io = std::io::Error::other("boom");
        let e: OzmaError = io.into();
        assert!(matches!(e, OzmaError::Io(_)));
    }
}
