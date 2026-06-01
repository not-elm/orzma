use bevy::prelude::*;

/// The label enum labeling the types of systems in Ozmux
#[derive(Debug, Hash, PartialEq, Eq, Clone, SystemSet)]
pub enum OzmuxSystems {
    /// Flags sessions needing a UI rebuild after an in-pane activity add /
    /// active-activity switch (which do not change `LayoutCells`). Runs after
    /// the control-bridge drain and before `SessionUi` so the inserted
    /// `SessionUiDirty` marker is visible to the rebuild the same frame.
    ChromeInvalidate,
    /// The phase for building the session UI.
    SessionUi,
    /// The phase that setup activities.
    SetupActivity,
    /// Per-frame input handling — keyboard and mouse. Members run in
    /// `crate::input::InputPhase` order: `Hover` → `Dispatch` →
    /// `FocusedKey`. The chain ensures click-to-focus (Dispatch)
    /// mutates `Session::active_pane` before the keyboard chord
    /// dispatcher (FocusedKey) reads it in the same frame.
    Input,
}
