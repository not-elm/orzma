//! Domain errors for the session layer.
//!
//! NOTE: Real ID types (`SessionId`, `PaneId`, `CellId`) move in Task 3.
//! Until then this file defines a placeholder string-based error.

use thiserror::Error;

#[derive(Error, Debug, Clone)]
pub enum SessionError {
    #[error("session not found session-id={0}")]
    SessionNotFound(String),
    #[error("pane not found pane-id={0}")]
    PaneNotFound(String),
    #[error("cell not found id={0}")]
    CellNotFound(String),
    #[error("invalid node type node-id={0}")]
    InvalidCellType(String),
    #[error("split target equals new_cell: cell-id={0}")]
    SplitTargetEqualsNewCell(String),
    #[error("cannot close the last pane in a session: pane-id={0}")]
    CannotCloseLastPane(String),
}

pub type SessionResult<T = ()> = Result<T, SessionError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_not_found_carries_id_in_message() {
        let id = "some-id".to_string();
        let err = SessionError::SessionNotFound(id.clone());
        assert!(err.to_string().contains(&id));
    }
}
