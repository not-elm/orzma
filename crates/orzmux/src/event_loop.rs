//! The multiplexer's thread-facing half: the command vocabulary the GUI
//! sends, and the loop that waits on the command channel and every
//! pane's PTY streams.

use crate::backend::{NewPaneAt, PaneDirection, PaneId, PaneTarget, RequestId, SplitId};
use orzma_tty::prelude::{MouseReport, TerminalKey, TerminalModifiers, WheelInput};
use orzma_tty::{CellPixels, EnvKey, EnvValue};
use orzma_vt::prelude::{
    CellSide, GridColumn, GridPoint, GridSize, InstanceId, PlacementSize, ScreenLine, Scroll,
    SelectionKind,
};
use std::path::PathBuf;

/// A command the GUI sends to the backend.
#[derive(Debug, Clone)]
pub enum OrzmuxCommand {
    /// The whole window's size in cells plus the cell pixel pitch.
    Resize {
        /// The window's size in cells.
        size: GridSize,
        /// The pixel size of one cell, used to derive the PTY winsize.
        cell_px: CellPixels,
    },
    /// Spawn a pane. `env` is forwarded to the shell verbatim.
    ///
    /// A split with `cwd: None` starts in the target pane's working
    /// directory: on Unix the directory of its foreground process or
    /// shell when the OS reports one, else the directory it last
    /// reported through OSC 7 or OSC 9;9, else the directory it was
    /// spawned in; on Windows the report is preferred over the OS. Only
    /// a directory that still exists and can be entered is used. A root
    /// pane with `cwd: None`, or a split whose target has none of these,
    /// starts in the user's home directory.
    NewPane {
        /// The id the resulting `PaneOpened` / `SpawnFailed` correlates to.
        request: RequestId,
        /// Where the new pane goes in the layout tree.
        at: NewPaneAt,
        /// The working directory to spawn the shell in, when given.
        cwd: Option<PathBuf>,
        /// Extra environment variables forwarded to the shell.
        env: Vec<(EnvKey, EnvValue)>,
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
    /// Sets whether the primary window has keyboard focus. The active pane
    /// holds focus only while the window does, and the window counts as
    /// focused until the first `WindowFocus` arrives.
    WindowFocus {
        /// The focus state to apply.
        focused: bool,
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
    /// Forward a mouse report to a pane's PTY. The pane writes nothing
    /// while its VT has no mouse tracking level in force.
    MouseInput {
        /// The pane receiving the mouse event.
        pane: PaneId,
        /// The mouse report to encode.
        report: MouseReport,
    },
    /// Route one frame's wheel notches over a pane by the pane's live VT
    /// modes.
    Wheel {
        /// The pane under the cursor.
        pane: PaneId,
        /// The notches and the modifiers and cell they were gathered with.
        input: WheelInput,
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
    /// Move a split's divider.
    ResizeSplit {
        /// The split whose divider moves.
        split: SplitId,
        /// The whole-window cell boundary to put the divider on: `x` for
        /// a vertical split, `y` for a horizontal one.
        position: u16,
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

impl OrzmuxCommand {
    /// The variant's name, as the refusal log line prints it.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Resize { .. } => "Resize",
            Self::NewPane { .. } => "NewPane",
            Self::KillPane { .. } => "KillPane",
            Self::SelectPane { .. } => "SelectPane",
            Self::SelectPaneDirection { .. } => "SelectPaneDirection",
            Self::WindowFocus { .. } => "WindowFocus",
            Self::KeyInput { .. } => "KeyInput",
            Self::Paste { .. } => "Paste",
            Self::MouseInput { .. } => "MouseInput",
            Self::Wheel { .. } => "Wheel",
            Self::Scroll { .. } => "Scroll",
            Self::SelectionStart { .. } => "SelectionStart",
            Self::SelectionUpdate { .. } => "SelectionUpdate",
            Self::SelectionClear { .. } => "SelectionClear",
            Self::CopySelection { .. } => "CopySelection",
            Self::RemovePlacements { .. } => "RemovePlacements",
            Self::ResizeSplit { .. } => "ResizeSplit",
            Self::MountPlacement { .. } => "MountPlacement",
        }
    }

    /// The pane the command addresses, or `None` when it addresses the
    /// window rather than one pane.
    pub fn target(&self) -> Option<PaneTarget> {
        match self {
            Self::KillPane { pane }
            | Self::KeyInput { pane, .. }
            | Self::Paste { pane, .. }
            | Self::CopySelection { pane } => Some(*pane),
            Self::SelectPane { pane } => Some(PaneTarget::Id(*pane)),
            Self::MouseInput { pane, .. }
            | Self::Wheel { pane, .. }
            | Self::Scroll { pane, .. }
            | Self::SelectionStart { pane, .. }
            | Self::SelectionUpdate { pane, .. }
            | Self::SelectionClear { pane }
            | Self::RemovePlacements { pane, .. }
            | Self::MountPlacement { pane, .. } => Some(PaneTarget::Id(*pane)),
            Self::Resize { .. }
            | Self::NewPane { .. }
            | Self::SelectPaneDirection { .. }
            | Self::WindowFocus { .. }
            | Self::ResizeSplit { .. } => None,
        }
    }
}
