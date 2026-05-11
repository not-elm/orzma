//! Domain errors for the session layer.

use crate::{cells::CellId, pane::PaneId, session::SessionId, window::WindowId};
use thiserror::Error;

#[derive(Error, Debug, Clone)]
pub enum MultiplexerError {
    #[error("session not found session-id={0}")]
    SessionNotFound(SessionId),

    #[error("window not found window-id={0}")]
    WindowNotFound(WindowId),

    #[error("pane {pane} does not belong to window {window}")]
    PaneNotInWindow { window: WindowId, pane: PaneId },

    #[error("window {window_id} does not belong to session {session_id}")]
    WindowDoesNotBelongToSession {
        session_id: SessionId,
        window_id: WindowId,
    },

    #[error("missiong parent cell")]
    MissingParentCell,

    #[error("pane not found pane-id={0}")]
    PaneNotFound(PaneId),

    #[error("cell not found id={0}")]
    CellNotFound(CellId),

    #[error("cell mapping not found for pane-id={0}")]
    CellForPaneNotFound(PaneId),

    #[error("cannot close the last pane under root: cell-id={0}")]
    CannotCloseLastPane(CellId),

    #[error("invalid node type node-id={0}")]
    InvalidCellType(CellId),

    #[error("split target equals new_cell: cell-id={0}")]
    SplitTargetEqualsNewCell(CellId),

    #[error("cannot close the last window in a session: session-id={0}")]
    CannotCloseLastWindow(SessionId),

    #[error("cannot close the last pane in a window: window-id={0}")]
    CannotCloseLastPaneInWindow(WindowId),

    #[error("active pane {pane_id} must belong to window {window_id}")]
    ActivePaneMustBelongToWindow {
        window_id: WindowId,
        pane_id: PaneId,
    },

    #[error("window {0} is not attached to any session")]
    WindowNotAttachedToSession(WindowId),

    #[error("activity not found: {0}")]
    ActivityNotFound(crate::activity::ActivityId),

    #[error("pane already placed in cell tree: {0}")]
    PaneAlreadyPlaced(crate::pane::PaneId),

    #[error("window not found for pane pane-id={0}")]
    WindowNotFoundForPane(PaneId),
}

pub type MultiplexerResult<T = ()> = Result<T, MultiplexerError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_not_found_carries_id_in_message() {
        let id = SessionId::new();
        let err = MultiplexerError::SessionNotFound(id.clone());
        assert!(err.to_string().contains(id.as_ref()));
    }

    #[test]
    fn window_not_found_carries_id_in_message() {
        let id = WindowId::new();
        let err = MultiplexerError::WindowNotFound(id.clone());
        assert!(err.to_string().contains(id.as_ref()));
    }

    #[test]
    fn cannot_close_last_window_carries_session_id() {
        let sid = SessionId::new();
        let err = MultiplexerError::CannotCloseLastWindow(sid.clone());
        assert!(err.to_string().contains(sid.as_ref()));
    }

    #[test]
    fn cannot_close_last_pane_in_window_carries_window_id() {
        let wid = WindowId::new();
        let err = MultiplexerError::CannotCloseLastPaneInWindow(wid.clone());
        assert!(err.to_string().contains(wid.as_ref()));
    }

    #[test]
    fn window_not_attached_carries_window_id() {
        let wid = WindowId::new();
        let err = MultiplexerError::WindowNotAttachedToSession(wid.clone());
        assert!(err.to_string().contains(wid.as_ref()));
    }
}
