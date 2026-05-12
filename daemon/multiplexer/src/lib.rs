pub mod error;
pub mod session;
pub mod window;

pub use error::{MultiplexerError, MultiplexerResult};
pub use session::{Session, SessionId, SessionState};
pub use window::{
    Activity, ActivityId, ActivityKind, Cell, CellId, CloseOutcome, LayoutCellState, Pane,
    PaneCell, PaneId, PaneState, RootCell, SetActiveOutcome, Side, SplitCell, SplitOrientation,
    Window, WindowId, WindowState,
};

/// Backwards-compatible alias for the active-pane outcome. Use
/// `SetActiveOutcome` directly in new code.
pub type SetActivePaneOutcome = SetActiveOutcome;
