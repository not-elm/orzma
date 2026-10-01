//! The error type the multiplexer reports, and the result alias built
//! on it.

use crate::backend::PaneId;
use orzma_tty::prelude::OrzmaTtyError;
use orzma_vt::prelude::VtError;
use orzma_webview_host::prelude::WebviewHostError;
use std::io::Error as IoError;
use thiserror::Error;

/// A `Result` whose error is [`OrzmuxError`].
pub type OrzmuxResult<T = ()> = Result<T, OrzmuxError>;

/// Every failure the multiplexer reports.
#[derive(Debug, Error)]
pub enum OrzmuxError {
    /// A pane was requested before the window reported its size.
    #[error("a pane was requested before the window reported its size")]
    NoGeometry,
    /// No live pane matches the target a command named.
    #[error("no pane matches the target")]
    UnresolvedTarget,
    /// No tab matches the target.
    #[error("no tab matches the target")]
    UnresolvedTab,
    /// A split was refused because the target is missing, or cannot
    /// hold two minimum leaves and a separator along the split axis.
    #[error("the target pane has too little room to divide")]
    SplitRefused,
    /// The new pane has no rectangle: its tab is gone, or the tab's tree
    /// does not hold the pane.
    #[error("the new pane has no rectangle in its tab's layout")]
    NoPaneRect,
    /// A VT operation for the new pane failed.
    #[error(transparent)]
    Vt(#[from] VtError),
    /// The shell for a new pane could not be spawned.
    #[error(transparent)]
    SpawnShell(#[from] OrzmaTtyError),
    /// The backend thread could not be started.
    #[error("the orzma-mux thread could not be started: {0}")]
    BackendThread(#[source] IoError),
    /// A pane's PTY refused a write.
    #[error("the pane refused a write: {source}")]
    PtyWrite {
        /// The pane whose PTY refused the write.
        pane: PaneId,
        /// The refusal itself.
        #[source]
        source: OrzmaTtyError,
    },
    /// The webview host refused a request or failed.
    #[error(transparent)]
    WebviewHost(#[from] WebviewHostError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use orzma_webview_host::prelude::Refusal;

    /// Asserts that a backend thread start-up failure's message includes
    /// the OS error text.
    ///
    /// Case: the OS refuses to start the `orzma-mux` thread.
    #[test]
    fn a_backend_thread_failure_keeps_the_os_error_text() {
        let io_err = IoError::other("out of threads");
        assert_eq!(
            OrzmuxError::BackendThread(io_err).to_string(),
            "the orzma-mux thread could not be started: out of threads"
        );
    }

    /// Asserts that each way a pane request can be refused reports its
    /// own reason, rather than all of them collapsing to one message.
    ///
    /// Case: a user asks for a pane before the window has reported its
    /// size, names a pane that is gone, and splits a pane that has no room
    /// left to divide.
    #[test]
    fn a_refused_pane_request_names_the_reason_it_was_refused() {
        assert_eq!(
            OrzmuxError::NoGeometry.to_string(),
            "a pane was requested before the window reported its size"
        );
        assert_eq!(
            OrzmuxError::UnresolvedTarget.to_string(),
            "no pane matches the target"
        );
        assert_eq!(
            OrzmuxError::SplitRefused.to_string(),
            "the target pane has too little room to divide"
        );
    }

    /// Asserts that a webview host failure keeps the host's own message.
    ///
    /// Case: a program asks to focus another program's page and the refusal
    /// is logged by the multiplexer.
    #[test]
    fn a_webview_host_failure_keeps_the_hosts_message() {
        let error = OrzmuxError::from(WebviewHostError::from(Refusal::NotOwner));
        assert_eq!(error.to_string(), "another connection owns the target");
    }
}
