//! The error type the multiplexer reports, and the result alias built
//! on it.

use orzma_tty::prelude::OrzmaTtyError;
use orzma_vt::prelude::VtError;
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
    /// A root pane was requested while the tree already holds one.
    #[error("the tree already holds a root pane")]
    RootOccupied,
    /// A split was refused because the target is missing, or cannot
    /// hold two minimum leaves and a separator along the split axis.
    #[error("the target pane has too little room to divide")]
    SplitRefused,
    /// The new pane is absent from the layout the tree solved for it.
    #[error("the new pane is not in the solved layout")]
    Unsolved,
    /// A VT operation for the new pane failed.
    #[error(transparent)]
    Vt(#[from] VtError),
    /// The shell for a new pane could not be spawned.
    #[error(transparent)]
    SpawnShell(#[from] OrzmaTtyError),
    /// The backend thread could not be started.
    #[error("the orzma-mux thread could not be started: {0}")]
    BackendThread(#[source] IoError),
}
