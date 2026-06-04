//! Domain errors for the multiplexer layer. Every variant carries a Bevy
//! `Entity` (the crate is Entity-addressed) rather than a typed ID.

use bevy::ecs::entity::Entity;
use thiserror::Error;

use crate::cells::CellId;

/// Domain errors returned by multiplexer operations.
#[derive(Error, Debug, Clone)]
pub enum MultiplexerError {
    #[error("workspace entity not found: {0:?}")]
    WorkspaceNotFound(Entity),

    #[error(
        "workspace {0:?} has no cached dimensions; the renderer must set dimensions before resize-pane"
    )]
    WorkspaceNotMeasured(Entity),

    #[error("pane {pane:?} does not belong to workspace {workspace:?}")]
    PaneNotInWorkspace { workspace: Entity, pane: Entity },

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

    #[error("cannot close the last pane in workspace {0:?}")]
    CannotCloseLastPaneInWorkspace(Entity),

    #[error("active pane {pane:?} must belong to workspace {workspace:?}")]
    ActivePaneMustBelongToWorkspace { workspace: Entity, pane: Entity },

    #[error("surface entity not found: {0:?}")]
    SurfaceNotFound(Entity),

    #[error("pane already placed in cell tree: {0:?}")]
    PaneAlreadyPlaced(Entity),

    #[error("surface {surface:?} is not in pane {pane:?}")]
    SurfaceNotInPane { pane: Entity, surface: Entity },

    #[error("cannot remove the only surface in pane {0:?}")]
    CannotRemoveLastSurface(Entity),
}

/// Result alias used throughout the multiplexer crate.
pub type MultiplexerResult<T = ()> = Result<T, MultiplexerError>;
