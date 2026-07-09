//! Clipboard action modules: the copy write seam and the paste pipeline.

use crate::action::clipboard::copy::ClipboardCopyActionPlugin;
use bevy::prelude::*;

mod copy;
pub mod paste;

pub(crate) use copy::CopyAction;
#[cfg(test)]
pub(crate) use copy::test_support;

/// Aggregates the per-feature clipboard action plugins.
pub(super) struct ClipboardActionsPlugin;

impl Plugin for ClipboardActionsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((ClipboardCopyActionPlugin,));
    }
}
