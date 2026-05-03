use thiserror::Error;

use crate::session::{cell::CellId, pane::PaneId};

pub type OzmuxResult<T = ()> = Result<T, OzmuxError>;

#[derive(Error, Debug, Clone)]
pub enum OzmuxError {
    #[error("failed to launch daemon http server:{0}")]
    FailedLaunchHttpServer(String),

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
