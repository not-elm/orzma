//! ECS-native multiplexer for ozmux. Session, Pane, and Activity are Bevy
//! entities related by `ChildOf`. All mutations route through the
//! `MultiplexerCommands` SystemParam; the only observers handle dangling
//! `Entity` references when a child entity is despawned.
//!
//! No typed IDs (`SessionId` / `PaneId` / `ActivityId`) — every reference
//! is a Bevy `Entity`. Each entity also carries `Name` (from
//! `bevy::prelude::Name`) for tracing readability.

pub mod cells;
pub mod commands;
pub mod components;
pub mod direction;
pub mod error;
pub mod observers;
pub mod plugin;
pub mod resize;
pub mod swap;

pub use cells::{
    Cell, CellId, CloseOutcome, LayoutCellState, PaneCell, Rect, RootCell, Side, SplitCell,
    SplitOrientation,
};
pub use commands::{MultiplexerCommands, SessionCreated, SplitOutcome};
pub use components::{
    ActiveActivity, ActivePane, ActivityKind, ActivityMarker, AttachedSession, BrowserProfile,
    CopyMode, ExtensionActivityAid, LayoutCells, PaneDimensions, PaneMarker, SessionDimensions,
    SessionMarker, SessionUiSubtree,
};
pub use direction::{CycleDirection, PaneDirection};
pub use error::{MultiplexerError, MultiplexerResult};
pub use plugin::MultiplexerPlugin;
pub use resize::ResizePaneOutcome;
pub use swap::{SwapOffset, SwapOutcome};
