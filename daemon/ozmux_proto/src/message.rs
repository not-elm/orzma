//! The client↔daemon wire messages (control plane).

use ozmux_mux::{
    MuxEvent, PaneDirection, PaneId, SessionSnapshot, Side, SplitOrientation, SurfaceId,
    SurfaceKind, SwapOffset, WorkspaceId,
};
use ozmux_vt::event::VtEvent;
use ozmux_vt::frame::Frame;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A 0-indexed viewport point (proto mirror of alacritty's viewport `Point`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewportPoint {
    /// 0-indexed viewport row (0 = top visible row).
    pub line: i32,
    /// 0-indexed column.
    pub col: usize,
}

/// Proto mirror of alacritty `index::Side` — which half of a cell a selection edge sits on.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum CellSide {
    /// The left half of the cell.
    Left,
    /// The right half of the cell.
    Right,
}

/// Proto mirror of alacritty `SelectionType`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum SelectionKind {
    /// A simple character-wise selection.
    Simple,
    /// A rectangular block selection.
    Block,
    /// A whole-line selection.
    Lines,
    /// A semantic (word-boundary) selection.
    Semantic,
}

/// Proto mirror of the subset of alacritty `ViMotion` the GUI emits in copy mode.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum ViMotionKind {
    /// Move one cell left.
    Left,
    /// Move one cell right.
    Right,
    /// Move one line up.
    Up,
    /// Move one line down.
    Down,
    /// Move to the first column of the line.
    First,
    /// Move to the last column of the line.
    Last,
    /// Move to the first occupied (non-blank) column of the line.
    FirstOccupied,
    /// Move to the top of the viewport.
    High,
    /// Move to the bottom of the viewport.
    Low,
    /// Move to the start of the next word.
    WordRight,
    /// Move to the start of the previous word.
    WordLeft,
    /// Move to the end of the next word.
    WordRightEnd,
}

/// A copy-mode / selection / scroll-back operation on a surface's VT.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum CopyModeOp {
    /// Enter vi-mode (the vi-cursor appears in subsequent frames).
    Enter,
    /// Leave vi-mode (clears selection + snaps to the live tail).
    Exit,
    /// Move the vi-cursor by a motion.
    ViMotion(ViMotionKind),
    /// Place the vi-cursor at a viewport point.
    ViGoto {
        /// The target viewport point.
        point: ViewportPoint,
    },
    /// Scroll one page toward history.
    ScrollPageUp,
    /// Scroll one page toward the tail.
    ScrollPageDown,
    /// Start a selection anchored at a point.
    SelectionStartAt {
        /// The anchor viewport point.
        point: ViewportPoint,
        /// Which half of the cell the anchor sits on.
        side: CellSide,
        /// The selection type.
        ty: SelectionKind,
    },
    /// Extend the active selection to a point (no-op if none).
    SelectionUpdateTo {
        /// The viewport point to extend to.
        point: ViewportPoint,
        /// Which half of the cell the extent sits on.
        side: CellSide,
    },
    /// Start a selection at the current vi-cursor.
    SelectionStart {
        /// The selection type.
        ty: SelectionKind,
    },
    /// Drop the active selection.
    SelectionClear,
    /// Change the active selection's type.
    SelectionChangeType {
        /// The new selection type.
        ty: SelectionKind,
    },
    /// Extract the selection's text -> `ServerMessage::SelectionCopied`.
    CopySelection,
}

/// A message from a client to the daemon.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ClientMessage {
    Health,
    /// Split a pane.
    Split {
        /// The pane to split.
        pane: PaneId,
        /// Split axis.
        orientation: SplitOrientation,
        /// Which side the new pane goes.
        side: Side,
        /// The new pane's initial surface kind.
        kind: SurfaceKind,
        /// Working directory for the new surface (None = inherit/default).
        cwd: Option<PathBuf>,
    },
    /// Set the active surface within a pane.
    SetActiveSurface {
        /// The surface to activate.
        surface: SurfaceId,
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
    /// Swap a pane with its prev/next neighbor in the layout.
    SwapPane {
        /// The pane to swap.
        pane: PaneId,
        /// Which neighbor to swap with.
        offset: SwapOffset,
    },
    /// Spawn a surface in a pane.
    SpawnSurface {
        /// The target pane.
        pane: PaneId,
        /// Kind of surface to spawn.
        kind: SurfaceKind,
        /// Working directory for the new surface (None = inherit/default).
        cwd: Option<PathBuf>,
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
    /// Pre-encoded input bytes for a surface (client→daemon).
    Input {
        /// The target surface.
        surface: SurfaceId,
        /// Encoded input bytes (keys/mouse via ozmux_vt::input/mouse).
        bytes: Vec<u8>,
    },
    /// Create a workspace in the active session (optionally named).
    CreateWorkspace {
        /// Optional explicit name; daemon assigns an auto-name when `None`.
        name: Option<String>,
    },
    /// Switch the active session's active workspace.
    SelectWorkspace {
        /// The workspace to make active.
        workspace: WorkspaceId,
    },
    /// Scroll a surface's viewport (positive delta = back into scrollback history, negative = toward the live tail).
    Scroll {
        /// The target surface.
        surface: SurfaceId,
        /// Signed row delta (positive scrolls back into history).
        delta: i32,
    },
    /// A copy-mode / selection op on a surface's VT.
    CopyMode {
        /// The target surface.
        surface: SurfaceId,
        /// The operation.
        op: CopyModeOp,
    },
    /// Ask the daemon to shut down cleanly (used by `ozmuxd --kill`).
    Shutdown,
}

/// A message from the daemon to a client.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ServerMessage {
    /// Handshake reply: the cold-attach snapshot.
    Welcome {
        /// Full session state at the moment of attach.
        snapshot: SessionSnapshot,
    },
    /// A batch of mux events from one command (folded atomically by the client).
    Events(Vec<MuxEvent>),
    /// A terminal frame (snapshot or delta) for a surface (daemon→client).
    Frame {
        /// The surface this frame belongs to.
        surface: SurfaceId,
        /// The VT frame.
        frame: Frame,
    },
    /// A per-surface VT control event (title/cwd/bell/clipboard/mode/child-exit).
    SurfaceEvent {
        /// The surface that raised it.
        surface: SurfaceId,
        /// The event.
        event: VtEvent,
    },
    /// A rejected command.
    Error {
        /// Human-readable reason.
        message: String,
    },
    /// The result of a `CopyModeOp::CopySelection`, delivered ONLY to the
    /// originating client (reliable, not the lossy frame path).
    SelectionCopied {
        /// The surface the selection came from.
        surface: SurfaceId,
        /// The extracted selection text.
        text: String,
    },
}
