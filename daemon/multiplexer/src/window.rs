//! Window module. A Window owns its cells, panes, and pane-to-cell index.
//!
//! Re-exports flatten the public surface so callers can still write
//! `ozmux_multiplexer::window::{Window, WindowId, PaneId, ActivityId, ...}`.

pub mod cells;
pub mod pane;
#[allow(clippy::module_inception)]
mod window;

pub use cells::{
    Cell, CellId, CloseOutcome, LayoutCellState, PaneCell, Rect, RootCell, Side, SplitCell,
    SplitOrientation,
};
pub use pane::activity::{Activity, ActivityId, ActivityKind};
pub use pane::{CycleDirection, Pane, PaneId, PaneState, SetActiveOutcome};
pub use window::{Window, WindowId, WindowState};
