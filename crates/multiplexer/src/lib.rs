//! ECS-native multiplexer for ozmux. Workspace, Pane, and Surface are Bevy
//! entities related by `ChildOf`. All mutations route through the
//! `MultiplexerCommands` SystemParam; active-pane and active-surface repointing
//! after despawns is handled authoritatively by `apply_event` in `mirror`.
//!
//! No typed IDs (`WorkspaceId` / `PaneId` / `SurfaceId`) — every reference
//! is a Bevy `Entity`. Each entity also carries `Name` (from
//! `bevy::prelude::Name`) for tracing readability.

pub mod commands;
pub mod components;
pub mod direction;
pub mod error;
pub mod layout;
pub mod mirror;
pub mod plugin;
pub mod resize;
pub mod swap;

pub use commands::{MultiplexerCommands, SplitOutcome, WorkspaceCreated, WorkspaceNameCounter};
pub use components::{
    ActivePane, ActiveSurface, AttachedWorkspace, BrowserProfile, CopyMode, Cwd,
    ExtensionSurfaceId, OwningExtension, OwningWorkspace, PaneDimensions, PaneMarker, SplitNode,
    SurfaceKind, SurfaceMarker, SurfaceOf, Surfaces, WorkspaceCreatedAt, WorkspaceDimensions,
    WorkspaceMarker, WorkspaceUiSubtree,
};
pub use direction::{CycleDirection, PaneDirection};
pub use error::{MultiplexerError, MultiplexerResult};
pub use layout::{Side, SplitOrientation, split_ratio};
pub use mirror::{
    MultiplexerStartupSet, MuxPaneId, MuxSplitId, MuxState, MuxSurfaceId, MuxWorkspaceId,
};
pub use plugin::MultiplexerPlugin;
pub use resize::ResizePaneOutcome;
pub use swap::{SwapOffset, SwapOutcome};
