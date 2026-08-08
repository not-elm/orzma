//! Inbound request `EntityEvent`s: commands the host UI fires AT a
//! terminal entity, as opposed to the outbound `Term*Signal`s in
//! `signals.rs` that are drained FROM the VT.

use crate::requests::{key_input::KeyInputPlugin, mouse_input::MouseInputPlugin};
use bevy::prelude::*;

mod key_input;
mod mouse_input;

pub use key_input::RequestTermKeyInput;
pub use mouse_input::RequestTermMouseInput;

pub(crate) struct OrzmaEventRequestPlugin;

impl Plugin for OrzmaEventRequestPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((KeyInputPlugin, MouseInputPlugin));
    }
}
