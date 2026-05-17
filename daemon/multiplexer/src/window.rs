//! Window module. A Window owns its cells, panes, and pane-to-cell index.
//!
//! Re-exports flatten the public surface so callers can still write
//! `ozmux_multiplexer::window::{Window, WindowId, PaneId, ActivityId, ...}`.

pub mod cells;
pub mod direction;
pub mod pane;
pub(crate) mod resize;
#[allow(clippy::module_inception)]
mod window;

pub use cells::{
    Cell, CellId, CloseOutcome, LayoutCellState, PaneCell, RootCell, Side, SplitCell,
    SplitOrientation,
};
pub use direction::PaneDirection;
pub use pane::activity::{Activity, ActivityId, ActivityKind, BrowserProfile};
pub use pane::{CycleDirection, Pane, PaneId, PaneState, SetActiveOutcome};
pub use window::{Window, WindowDimensions, WindowId, WindowState};
