use bevy::prelude::*;

/// The label enum labeling the types of systems in Ozmux
#[derive(Debug, Hash, PartialEq, Eq, Clone, SystemSet)]
pub enum OzmuxSystems {
    /// The phase for building the session UI.
    SessionUi,
    ///
    SetupActivity,
}
