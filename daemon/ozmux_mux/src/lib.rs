//! Bevy-free multiplexer core: the (future) source of truth for the
//! Session > Workspace > LayoutNode(Split | Pane) > Surface hierarchy.
//! Every mutation on `mux::Mux` returns a list of `event::MuxEvent`s;
//! the daemon serializes them to UDS and the Bevy mirror applies them.

pub mod direction;
pub mod error;
pub mod event;
pub mod geometry;
pub mod id;
pub mod mux;
pub mod snapshot;
pub mod surface;
pub mod tree;

pub use direction::{CycleDirection, PaneDirection, SwapOffset};
pub use error::{MuxError, MuxResult};
pub use event::{MuxEvent, SurfaceEntry};
pub use geometry::Rect;
pub use id::{NodeId, PaneId, SessionId, SplitId, SurfaceId, WorkspaceId};
pub use mux::Multiplexer;
pub use snapshot::{PaneSnapshot, SessionSnapshot, SurfaceState, WorkspaceSnapshot};
pub use surface::{BrowserProfile, Surface, SurfaceKind};
pub use tree::{LayoutNode, Pane, Side, Split, SplitOrientation, collect_node_ids};
