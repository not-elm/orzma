//! Domain errors for the multiplexer core. Plain enum (no `thiserror`),
//! matching the lean `ozmux_vt` sibling. Only variants that the API
//! actually constructs are kept.

use crate::id::{PaneId, SurfaceId, WorkspaceId};
use std::fmt;

/// Errors returned by `crate::mux::Mux` operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MuxError {
    /// No such workspace (or a stale id).
    WorkspaceNotFound(WorkspaceId),
    /// No such pane (or a stale id).
    PaneNotFound(PaneId),
    /// No such surface (or a stale id).
    SurfaceNotFound(SurfaceId),
    /// Refused: closing the workspace's only pane.
    CannotCloseLastPaneInWorkspace(WorkspaceId),
    /// Refused: removing a pane's only surface.
    CannotRemoveLastSurface(PaneId),
    /// A required parent node was absent (broken tree invariant).
    MissingParentCell,
}

impl fmt::Display for MuxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MuxError::WorkspaceNotFound(w) => write!(f, "workspace not found: {w:?}"),
            MuxError::PaneNotFound(p) => write!(f, "pane not found: {p:?}"),
            MuxError::SurfaceNotFound(s) => write!(f, "surface not found: {s:?}"),
            MuxError::CannotCloseLastPaneInWorkspace(w) => {
                write!(f, "cannot close the last pane in workspace {w:?}")
            }
            MuxError::CannotRemoveLastSurface(p) => {
                write!(f, "cannot remove the only surface in pane {p:?}")
            }
            MuxError::MissingParentCell => write!(f, "missing parent cell"),
        }
    }
}

impl std::error::Error for MuxError {}

/// Result alias for multiplexer operations.
pub type MuxResult<T = ()> = Result<T, MuxError>;
