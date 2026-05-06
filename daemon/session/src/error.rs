//! Domain errors for the session layer.

use crate::{SessionId, cell::CellId, pane::PaneId};
use thiserror::Error;

#[derive(Error, Debug, Clone)]
pub enum SessionError {
    #[error("session not found session-id={0}")]
    SessionNotFound(SessionId),

    #[error("pane not found pane-id={0}")]
    PaneNotFound(PaneId),

    #[error("cell not found id={0}")]
    CellNotFound(CellId),

    #[error("invalid node type node-id={0}")]
    InvalidCellType(CellId),

    #[error("split target equals new_cell: cell-id={0}")]
    SplitTargetEqualsNewCell(CellId),

    #[error("cannot close the last pane in a session: pane-id={0}")]
    CannotCloseLastPane(PaneId),
}

pub type SessionResult<T = ()> = Result<T, SessionError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_not_found_carries_id_in_message() {
        let id = SessionId::new();
        let err = SessionError::SessionNotFound(id.clone());
        assert!(err.to_string().contains(id.as_ref()));
    }
}
