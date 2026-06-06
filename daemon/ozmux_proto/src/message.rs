//! The client↔daemon wire messages (control plane).

use ozmux_mux::{
    MuxEvent, PaneDirection, PaneId, SessionSnapshot, Side, SplitOrientation, SurfaceId,
    SurfaceKind, WorkspaceId,
};
use serde::{Deserialize, Serialize};

/// A message from a client to the daemon.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ClientMessage {
    /// Attach handshake: protocol version + the client's usable viewport in
    /// cells (chrome already deducted client-side).
    Hello {
        /// Wire protocol version the client speaks.
        protocol_version: u32,
        /// Usable viewport in `(cols, rows)` cells.
        viewport: (u16, u16),
    },
    /// Split a pane.
    Split {
        /// The pane to split.
        pane: PaneId,
        /// Axis for the new split.
        orientation: SplitOrientation,
    },
    /// Close a pane.
    Close {
        /// The pane to close.
        pane: PaneId,
    },
    /// Move focus to a neighbor.
    Navigate {
        /// The currently focused pane.
        pane: PaneId,
        /// Cardinal direction to move focus.
        direction: PaneDirection,
    },
    /// Set the active pane.
    SetActivePane {
        /// The owning workspace.
        workspace: WorkspaceId,
        /// The pane to activate.
        pane: PaneId,
    },
    /// Spawn a surface in a pane.
    SpawnSurface {
        /// The target pane.
        pane: PaneId,
        /// Kind of surface to spawn.
        kind: SurfaceKind,
    },
    /// Break a surface out into a new pane.
    BreakSurfaceToPane {
        /// The surface to move.
        surface: SurfaceId,
        /// Split axis for the new pane.
        orientation: SplitOrientation,
        /// Side of the existing pane the new one lands on.
        side: Side,
    },
    /// Report a new usable viewport (client resized).
    SetViewport {
        /// New column count.
        cols: u16,
        /// New row count.
        rows: u16,
    },
}

/// A message from the daemon to a client.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ServerMessage {
    /// Handshake reply: version + the cold-attach snapshot.
    Welcome {
        /// Wire protocol version the daemon speaks.
        protocol_version: u32,
        /// Full session state at the moment of attach.
        snapshot: SessionSnapshot,
    },
    /// A state-change delta.
    Event(MuxEvent),
    /// A rejected command.
    Error {
        /// Human-readable reason.
        message: String,
    },
}
