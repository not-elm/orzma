//! Clipboard action modules: the copy write seam and the paste pipeline.

use crate::action::clipboard::{
    copy::ClipboardCopyActionPlugin, paste::ClipboardPasteActionPlugin,
};
use bevy::prelude::*;

mod copy;
mod paste;

pub(crate) use copy::CopyAction;
pub(crate) use paste::PasteAction;

/// Aggregates the per-feature clipboard action plugins.
pub(super) struct ClipboardActionsPlugin;

impl Plugin for ClipboardActionsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((ClipboardCopyActionPlugin, ClipboardPasteActionPlugin));
    }
}
