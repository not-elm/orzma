use bevy::prelude::*;

/// The label enum labeling the types of systems in Ozmux
#[derive(Debug, Hash, PartialEq, Eq, Clone, SystemSet)]
pub enum OzmuxSystems {
    /// Flags workspaces needing a UI rebuild after an in-pane surface add /
    /// active-surface switch (which do not change `LayoutCells`). Runs after
    /// the control-bridge drain and before `WorkspaceUi` so the inserted
    /// `WorkspaceUiDirty` marker is visible to the rebuild the same frame.
    ChromeInvalidate,
    /// The phase for building the workspace UI.
    WorkspaceUi,
    /// The phase that setup surfaces.
    SetupSurface,
    /// Per-frame input handling — keyboard and mouse. Members run in
    /// `crate::input::InputPhase` order: `Hover` → `Dispatch` →
    /// `FocusedKey`. The chain ensures click-to-focus (Dispatch)
    /// mutates `Workspace::active_pane` before the keyboard chord
    /// dispatcher (FocusedKey) reads it in the same frame.
    Input,
}
