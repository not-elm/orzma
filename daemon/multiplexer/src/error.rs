//! Domain errors for the session layer.

use crate::session::SessionId;
use crate::session::cells::CellId;
use crate::session::pane::PaneId;
use crate::session::pane::activity::ActivityId;
use thiserror::Error;

#[derive(Error, Debug, Clone)]
pub enum MultiplexerError {
    #[error("session not found session-id={0}")]
    SessionNotFound(SessionId),

    #[error(
        "session {0} has no cached dimensions; the renderer must set dimensions before resize-pane"
    )]
    SessionNotMeasured(SessionId),

    #[error("pane {pane} does not belong to session {session}")]
    PaneNotInSession { session: SessionId, pane: PaneId },

    #[error("missing parent cell")]
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

    #[error("cannot close the last pane in a session: session-id={0}")]
    CannotCloseLastPaneInSession(SessionId),

    #[error("active pane {pane_id} must belong to session {session_id}")]
    ActivePaneMustBelongToSession {
        session_id: SessionId,
        pane_id: PaneId,
    },

    #[error("activity not found: {0}")]
    ActivityNotFound(ActivityId),

    #[error("pane already placed in cell tree: {0}")]
    PaneAlreadyPlaced(PaneId),

    #[error("session not found for pane pane-id={0}")]
    SessionNotFoundForPane(PaneId),

    #[error("pane id conflict: {0}")]
    PaneIdConflict(PaneId),

    #[error("activity id conflict: {0}")]
    ActivityIdConflict(ActivityId),

    #[error("activity {activity} is not in pane {pane}")]
    ActivityNotInPane { pane: PaneId, activity: ActivityId },

    #[error("cannot remove the only activity in pane {0}")]
    CannotRemoveLastActivity(PaneId),

    #[error("pane {pane} claimed to be in session {claimed} but is actually in {actual}")]
    PaneAttachmentMismatch {
        pane: PaneId,
        claimed: SessionId,
        actual: SessionId,
    },
}

pub type MultiplexerResult<T = ()> = Result<T, MultiplexerError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_not_found_carries_id_in_message() {
        let id = SessionId(7);
        let err = MultiplexerError::SessionNotFound(id);
        assert!(err.to_string().contains("7"));
    }

    #[test]
    fn cannot_close_last_pane_in_session_carries_session_id() {
        let sid = SessionId(42);
        let err = MultiplexerError::CannotCloseLastPaneInSession(sid);
        assert!(err.to_string().contains("42"));
    }

    #[test]
    fn cannot_remove_last_activity_carries_pane_id() {
        let pid = PaneId::new();
        let err = MultiplexerError::CannotRemoveLastActivity(pid.clone());
        assert!(err.to_string().contains(pid.as_ref()));
    }
}
