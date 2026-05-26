//! Session module. A Session owns its cells, panes, and pane-to-cell index.
//!
//! Re-exports flatten the public surface so callers can write
//! `ozmux_multiplexer::session::{Session, SessionId, PaneId, ActivityId, ...}`.

pub mod cells;
pub mod direction;
pub mod pane;
pub(crate) mod resize;
#[allow(clippy::module_inception)]
mod session;
pub(crate) mod swap;

pub use cells::{
    Cell, CellId, CloseOutcome, LayoutCellState, PaneCell, RootCell, Side, SplitCell,
    SplitOrientation,
};
pub use direction::PaneDirection;
pub use pane::activity::{Activity, ActivityId, ActivityKind, BrowserProfile};
pub use pane::{CycleDirection, Pane, PaneId, PaneState, SetActiveOutcome};
pub use resize::ResizePaneOutcome;
pub use session::{Session, SessionDimensions, SessionId};
pub use swap::{SwapOffset, SwapOutcome};
