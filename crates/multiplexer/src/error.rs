//! Domain errors for the multiplexer layer. Adapted from the old
//! `daemon/multiplexer` error enum; every variant that previously carried
//! a typed ID (`SessionId` / `PaneId` / `ActivityId`) now carries a Bevy
//! `Entity` instead, since the new crate is Entity-addressed.

use bevy::ecs::entity::Entity;
use thiserror::Error;

use crate::cells::CellId;

/// Domain errors returned by multiplexer operations.
#[derive(Error, Debug, Clone)]
pub enum MultiplexerError {
    #[error("session entity not found: {0:?}")]
    SessionNotFound(Entity),

    #[error(
        "session {0:?} has no cached dimensions; the renderer must set dimensions before resize-pane"
    )]
    SessionNotMeasured(Entity),

    #[error("pane {pane:?} does not belong to session {session:?}")]
    PaneNotInSession { session: Entity, pane: Entity },

    #[error("missing parent cell")]
    MissingParentCell,

    #[error("pane entity not found: {0:?}")]
    PaneNotFound(Entity),

    #[error("cell not found: {0}")]
    CellNotFound(CellId),

    #[error("cell mapping not found for pane: {0:?}")]
    CellForPaneNotFound(Entity),

    #[error("cannot close the last pane under root: cell {0}")]
    CannotCloseLastPane(CellId),

    #[error("invalid node type: cell {0}")]
    InvalidCellType(CellId),

    #[error("split target equals new_cell: {0}")]
    SplitTargetEqualsNewCell(CellId),

    #[error("cannot close the last pane in session {0:?}")]
    CannotCloseLastPaneInSession(Entity),

    #[error("active pane {pane:?} must belong to session {session:?}")]
    ActivePaneMustBelongToSession { session: Entity, pane: Entity },

    #[error("activity entity not found: {0:?}")]
    ActivityNotFound(Entity),

    #[error("pane already placed in cell tree: {0:?}")]
    PaneAlreadyPlaced(Entity),

    #[error("activity {activity:?} is not in pane {pane:?}")]
    ActivityNotInPane { pane: Entity, activity: Entity },

    #[error("cannot remove the only activity in pane {0:?}")]
    CannotRemoveLastActivity(Entity),
}

/// Result alias used throughout the multiplexer crate.
pub type MultiplexerResult<T = ()> = Result<T, MultiplexerError>;
