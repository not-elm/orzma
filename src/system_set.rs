use bevy::prelude::*;

/// The label enum labeling the types of systems in Ozmux
#[derive(Debug, Hash, PartialEq, Eq, Clone, SystemSet)]
pub enum OzmuxSystems {
    /// The phase for building the session UI.
    SessionUi,
    /// The phase that updates per-pane activity chrome (tab-bar contents +
    /// active-host mounting + veil) in reaction to `Changed<Children>` /
    /// `Changed<ActiveActivity>`. Runs after `SessionUi` (so the stable chrome
    /// containers are reparented under the new pane frame) and before
    /// `SetupActivity`.
    ActivityChrome,
    /// The phase that setup activities.
    SetupActivity,
    /// Per-frame input handling — keyboard and mouse. Members run in
    /// `crate::input::InputPhase` order: `Hover` → `Dispatch` →
    /// `FocusedKey`. The chain ensures click-to-focus (Dispatch)
    /// mutates `Session::active_pane` before the keyboard chord
    /// dispatcher (FocusedKey) reads it in the same frame.
    Input,
}
