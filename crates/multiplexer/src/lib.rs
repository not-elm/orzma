//! ECS-native multiplexer for ozmux. Workspace, Pane, and Surface are Bevy
//! entities related by `ChildOf`. All mutations route through the
//! `MultiplexerCommands` SystemParam; the only observers handle dangling
//! `Entity` references when a child entity is despawned.
//!
//! No typed IDs (`WorkspaceId` / `PaneId` / `SurfaceId`) — every reference
//! is a Bevy `Entity`. Each entity also carries `Name` (from
//! `bevy::prelude::Name`) for tracing readability.

pub mod commands;
pub mod components;
pub mod direction;
pub mod error;
pub mod layout;
pub mod observers;
pub mod plugin;
pub mod resize;
pub mod swap;

pub use commands::{MultiplexerCommands, SplitOutcome, WorkspaceCreated, WorkspaceNameCounter};
pub use components::{
    ActivePane, ActiveSurface, AttachedWorkspace, CopyMode, Cwd, ExtensionSurfaceId,
    OwningExtension, OwningWorkspace, PaneDimensions, PaneMarker, SplitNode, SurfaceMarker,
    SurfaceOf, Surfaces, WorkspaceCreatedAt, WorkspaceDimensions, WorkspaceMarker,
    WorkspaceUiSubtree,
};
pub use direction::{CycleDirection, PaneDirection, pane_in_direction};
pub use error::{MultiplexerError, MultiplexerResult};
pub use layout::{LayoutTree, Rect, Side, SplitOrientation, split_ratio};
pub use plugin::MultiplexerPlugin;
pub use resize::ResizePaneOutcome;
pub use swap::{SwapOffset, SwapOutcome};
