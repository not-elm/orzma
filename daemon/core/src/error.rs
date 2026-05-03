use thiserror::Error;

use crate::session::{cell::CellId, pane::PaneId, SessionId};

pub type OzmuxResult<T = ()> = Result<T, OzmuxError>;

#[derive(Error, Debug, Clone)]
pub enum OzmuxError {
    #[error("failed to launch daemon http server:{0}")]
    FailedLaunchHttpServer(String),

    #[error("session not found session-id={0}")]
    SessionNotFound(SessionId),

    #[error("pane not found pane-id={0}")]
    PaneNotfound(PaneId),

    #[error("cell not found id={0}")]
    CellNotfound(CellId),

    #[error("invalid node type node-id={0}")]
    InvalidCellType(CellId),

    #[error("split target equals new_cell: cell-id={0}")]
    SplitTargetEqualsNewCell(CellId),

    #[error("cannot close the last pane in a session: pane-id={0}")]
    CannotCloseLastPane(PaneId),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionId;

    #[test]
    fn session_not_found_carries_id_in_message() {
        let id = SessionId::new();
        let err = OzmuxError::SessionNotFound(id.clone());
        assert!(err.to_string().contains(id.as_ref()));
    }
}
