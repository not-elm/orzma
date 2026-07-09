//! Clipboard action modules: the copy write seam and the paste pipeline.

use crate::action::clipboard::{
    copy::ClipboardCopyActionPlugin, paste::ClipboardPasteActionPlugin,
};
use bevy::prelude::*;

mod copy;
mod paste;

pub(crate) use copy::CopyAction;
#[cfg(test)]
pub(crate) use copy::test_support;
pub(crate) use paste::build_paste_bytes;

/// Aggregates the per-feature clipboard action plugins.
pub(super) struct ClipboardActionsPlugin;

impl Plugin for ClipboardActionsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((ClipboardCopyActionPlugin, ClipboardPasteActionPlugin));
    }
}
