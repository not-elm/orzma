use bevy::prelude::*;

use crate::action::clipboard::copy::ClipboardCopyActionPlugin;

pub mod copy;
pub mod paste;

pub struct ClipboardActionsPlugin;

impl Plugin for ClipboardActionsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((ClipboardCopyActionPlugin,));
    }
}
