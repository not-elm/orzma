//! The wire vocabulary between the GUI and the multiplexer backend:
//! identifiers, the commands the GUI sends, the events the backend
//! emits, and the layout snapshot. Everything here is plain data (no
//! Bevy types, no GPU handles) so the transport can later become a
//! socket.

use orzma_tty::prelude::{CellPixels, MouseReport, TerminalKey, TerminalModifiers};
use orzma_vt::prelude::{
    CellSide, Frame, GridColumn, GridPoint, GridSize, InstanceId, PlacementSize, ScreenLine,
    Scroll, SelectionKind, VtSignal,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// A pane the backend minted. Never reused within one backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PaneId(pub u32);

/// A window (tab). PR-1 has exactly one, `WindowId(0)`; the type exists
/// so later protocol additions do not renumber anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct WindowId(pub u32);

/// A GUI-minted correlation id for a `NewPane` request, echoed by the
/// `PaneOpened` / `SpawnFailed` that answers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RequestId(pub u64);

impl RequestId {
    /// Mints the next id. Process-wide, strictly increasing.
    pub fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// The position of a command in the GUI's send order; `Layout.seq`
/// reports how far the backend has processed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct CommandSeq(pub u64);

/// Which pane a command addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneTarget {
    /// The backend's active pane at the moment the command is processed.
    Active,
    /// A specific pane.
    Id(PaneId),
}

/// Which way a split divides a pane, named after the divider the user
/// sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitOrientation {
    /// A vertical divider: the panes end up side by side.
    Vertical,
    /// A horizontal divider: the panes end up stacked.
    Horizontal,
}

/// A neighbour direction for directional pane selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneDirection {
    /// The pane to the left of the current one.
    Left,
    /// The pane below the current one.
    Down,
    /// The pane above the current one.
    Up,
    /// The pane to the right of the current one.
    Right,
}

/// Where a new pane goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewPaneAt {
    /// The first pane; valid only while the tree is empty.
    Root,
    /// Split `pane`, putting the new pane right of / below it.
    Split {
        /// The pane being split.
        pane: PaneTarget,
        /// The direction of the divider the split introduces.
        orientation: SplitOrientation,
    },
}

/// A command the GUI sends to the backend.
#[derive(Debug, Clone)]
pub enum OrzmuxCommand {
    /// The whole window's size in cells plus the cell pixel pitch.
    Resize {
        /// The window's width in cells.
        cols: u16,
        /// The window's height in cells.
        rows: u16,
        /// The pixel size of one cell, used to derive the PTY winsize.
        cell_px: CellPixels,
    },
    /// Spawn a pane. `cwd: None` inherits the split target's last
    /// reported directory. `env` is forwarded to the shell verbatim.
    NewPane {
        /// The id the resulting `PaneOpened` / `SpawnFailed` correlates to.
        request: RequestId,
        /// Where the new pane goes in the layout tree.
        at: NewPaneAt,
        /// The working directory to spawn the shell in, when given.
        /// Reserved for a client-chosen directory (tmux's `-c`); the
        /// host sends `None` today.
        cwd: Option<PathBuf>,
        /// Extra environment variables forwarded to the shell.
        env: Vec<(String, String)>,
    },
    /// Terminate a pane and remove it from the layout tree.
    KillPane {
        /// The pane to kill.
        pane: PaneTarget,
    },
    /// Make a pane the backend's active pane.
    SelectPane {
        /// The pane to activate.
        pane: PaneId,
    },
    /// Move the active pane to its neighbour in the given direction.
    SelectPaneDirection {
        /// The neighbour direction to select.
        direction: PaneDirection,
    },
    /// Forward a key press to a pane's PTY.
    KeyInput {
        /// The pane receiving the key.
        pane: PaneTarget,
        /// The key pressed.
        key: TerminalKey,
        /// The modifier keys held alongside `key`.
        mods: TerminalModifiers,
    },
    /// Forward pasted text to a pane.
    Paste {
        /// The pane receiving the paste.
        pane: PaneTarget,
        /// The pasted text.
        text: String,
    },
    /// Forward a mouse report to a pane's PTY.
    MouseInput {
        /// The pane receiving the mouse event.
        pane: PaneId,
        /// The mouse report to encode.
        report: MouseReport,
    },
    /// Scroll a pane's viewport.
    Scroll {
        /// The pane to scroll.
        pane: PaneId,
        /// The scroll motion to apply.
        scroll: Scroll,
    },
    /// Begin a selection in a pane.
    SelectionStart {
        /// The pane the selection starts in.
        pane: PaneId,
        /// The cell the selection anchors at.
        cell: GridPoint,
        /// Which half of the anchor cell the press landed on.
        side: CellSide,
        /// The selection's granularity (cell, word, line).
        kind: SelectionKind,
    },
    /// Extend an in-progress selection to a new cell.
    SelectionUpdate {
        /// The pane whose selection is extended.
        pane: PaneId,
        /// The cell the selection now extends to.
        cell: GridPoint,
        /// Which half of the target cell the drag landed on.
        side: CellSide,
    },
    /// Clear a pane's selection.
    SelectionClear {
        /// The pane whose selection is cleared.
        pane: PaneId,
    },
    /// Read back the text of a pane's current selection.
    CopySelection {
        /// The pane to read the selection from.
        pane: PaneTarget,
    },
    /// Release webview placement instances a pane no longer displays.
    RemovePlacements {
        /// The pane the placements belong to.
        pane: PaneId,
        /// The placement instances to release.
        instances: Vec<InstanceId>,
    },
    /// Register a host-driven webview placement at a visible cell of a
    /// pane — the socket-op counterpart of the APC `mount` for PTYs that
    /// drop APC (ConPTY).
    MountPlacement {
        /// The pane the placement belongs to.
        pane: PaneId,
        /// The host-minted instance the mount registers.
        instance: InstanceId,
        /// The visible row the rect's top edge sits on.
        row: ScreenLine,
        /// The column the rect's left edge sits on.
        column: GridColumn,
        /// The rect's extent in cells.
        size: PlacementSize,
    },
}

/// Why a pane closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    /// The shell exited; `code` is `None` when the wait itself failed.
    ChildExit {
        /// The shell's exit code, when the wait could observe it.
        code: Option<i32>,
    },
    /// The GUI killed it.
    Killed,
}

/// One pane's rectangle in whole-window cell coordinates, origin at the
/// window's top-left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneRect {
    /// The pane this rectangle belongs to.
    pub pane: PaneId,
    /// Left edge in cells from the window's left.
    pub x: u16,
    /// Top edge in cells from the window's top.
    pub y: u16,
    /// Width in cells.
    pub cols: u16,
    /// Height in cells.
    pub rows: u16,
}

/// A one-cell-wide divider between two panes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Separator {
    /// Whether the divider runs vertically or horizontally.
    pub orientation: SplitOrientation,
    /// Left edge in cells from the window's left.
    pub x: u16,
    /// Top edge in cells from the window's top.
    pub y: u16,
    /// The divider's length in cells.
    pub len: u16,
}

/// The complete pane geometry after one tree mutation or selection.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    /// The last command the backend processed before building this.
    pub seq: CommandSeq,
    /// The extent the panes tile: the window size in cells, widened per
    /// axis to the tree's minimum when the window is smaller.
    pub size: GridSize,
    /// The pane the backend considers active, when any pane exists.
    pub active: Option<PaneId>,
    /// Every pane's rectangle in the current tree.
    pub panes: Vec<PaneRect>,
    /// Every divider between adjacent panes in the current tree.
    pub separators: Vec<Separator>,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            seq: CommandSeq::default(),
            size: GridSize { cols: 0, rows: 0 },
            active: None,
            panes: Vec::new(),
            separators: Vec::new(),
        }
    }
}

/// An event the backend emits to the GUI.
#[derive(Debug, Clone, PartialEq)]
pub enum OrzmuxEvent {
    /// A pane requested by `NewPane` was spawned successfully.
    PaneOpened {
        /// The newly spawned pane.
        pane: PaneId,
        /// The `NewPane` request this answers.
        request: RequestId,
    },
    /// A pane requested by `NewPane` failed to spawn.
    SpawnFailed {
        /// The `NewPane` request this answers.
        request: RequestId,
        /// A human-readable description of the failure.
        error: String,
    },
    /// Exactly one per tree mutation or selection. `frames` carries the
    /// immediate repaint of every pane whose size changed.
    Layout {
        /// The new layout snapshot.
        layout: Layout,
        /// The immediate repaint of every pane whose size changed.
        frames: Vec<(PaneId, Frame)>,
    },
    /// A pane produced a new frame outside of a layout change.
    Frame {
        /// The pane that produced the frame.
        pane: PaneId,
        /// The pane's new frame.
        frame: Frame,
    },
    /// A pane's VT emitted a signal the GUI must act on.
    Signal {
        /// The pane the signal came from.
        pane: PaneId,
        /// The signal itself.
        signal: VtSignal,
    },
    /// Exactly one per `CopySelection`; `text` is `None` when the target
    /// could not be resolved or had no selection.
    SelectionText {
        /// The selection's text, when a selection existed.
        text: Option<String>,
    },
    /// A pane closed, whether by shell exit or by `KillPane`.
    PaneClosed {
        /// The pane that closed.
        pane: PaneId,
        /// Why the pane closed.
        reason: CloseReason,
    },
}

const fn assert_send_static<T: Send + 'static>() {}
const _: () = assert_send_static::<OrzmuxEvent>();
const _: () = assert_send_static::<OrzmuxCommand>();

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts that request ids mint strictly increasing values.
    ///
    /// Case: the GUI fires two pane spawns in one frame and must
    /// correlate each `PaneOpened` back to its own entity.
    #[test]
    fn request_ids_are_strictly_increasing() {
        let a = RequestId::next();
        let b = RequestId::next();
        assert!(a.0 < b.0);
    }

    /// Asserts that the default layout is the empty snapshot the GUI
    /// compares against to detect the session ending.
    ///
    /// Case: the GUI initializes `CurrentLayout` before the first pane.
    #[test]
    fn the_default_layout_has_no_panes() {
        let layout = Layout::default();
        assert!(layout.panes.is_empty());
        assert!(layout.separators.is_empty());
        assert_eq!(layout.active, None);
    }
}
