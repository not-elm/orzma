//! Aggregates the ozmux shortcut-action plugins that dispatch through
//! `EntityEvent`s. Currently wires the session-lifecycle actions
//! (`NewSession`, `FocusSession`, `FocusSessionNumber`) handled by
//! `session::OzmuxSessionActionPlugin`.

use bevy::prelude::*;
use session::OzmuxSessionActionPlugin;

pub(crate) mod session;

/// Bevy Plugin that registers every action-dispatch sub-plugin under the
/// `action` module.
pub(crate) struct OzmuxActionPlugin;

impl Plugin for OzmuxActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((OzmuxSessionActionPlugin,));
    }
}
