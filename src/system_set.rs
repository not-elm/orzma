use bevy::prelude::*;

/// The label enum labeling the types of systems in Ozmux
#[derive(Debug, Hash, PartialEq, Eq, Clone, SystemSet)]
pub enum OzmuxSystems {
    /// The phase for building the session UI.
    SessionUi,
    /// The phase that setup activities.
    SetupActivity,
    /// Per-frame input handling — keyboard and mouse. Members are
    /// ordered: `dispatch_mouse_buttons` runs `.before(dispatch_focused_key)`
    /// so click-to-focus mutates `Session::active_pane` before the
    /// keyboard chord dispatcher reads it in the same frame.
    Input,
}
