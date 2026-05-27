use bevy::prelude::*;

/// The label enum labeling the types of systems in Ozmux
#[derive(Debug, Hash, PartialEq, Eq, Clone, SystemSet)]
pub enum OzmuxSystems {
    SessionUi,
    SetupActivity,
}
