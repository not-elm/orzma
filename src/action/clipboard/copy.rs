//! Writes to the system clipboard, either placing text on it or
//! clearing it.

use bevy::{clipboard::ClipboardError, prelude::*};

/// Requests that `text` be written to the system clipboard.
#[derive(Event, Debug, Clone)]
pub(crate) struct CopyAction {
    /// The text to place on the system clipboard.
    pub text: String,
}

/// Requests that the system clipboard be cleared.
#[derive(Event, Debug, Clone)]
pub(super) struct ClearClipboardAction;

/// Adds orzma's clipboard write path onto Bevy's `Clipboard` resource,
/// provided by `DefaultPlugins`.
pub(super) struct ClipboardCopyActionPlugin;

impl Plugin for ClipboardCopyActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_copy).add_observer(on_clear_clipboard);
    }
}

fn on_copy(ev: On<CopyAction>, mut clipboard: ResMut<Clipboard>) {
    report_write(clipboard.set_text(ev.text.as_str()));
}

fn on_clear_clipboard(_ev: On<ClearClipboardAction>, mut clipboard: ResMut<Clipboard>) {
    // TODO: clear the clipboard instead of writing the empty string once
    // bevy_clipboard exposes a clear operation; arboard, which it wraps,
    // already has `Clipboard::clear`.
    report_write(clipboard.set_text(""));
}

fn report_write(result: Result<(), ClipboardError>) {
    match result {
        Ok(()) => {}
        Err(ClipboardError::ClipboardNotSupported) => {
            tracing::debug!(
                target: "orzma::clipboard",
                "clipboard write skipped: no system clipboard backend (headless)",
            );
        }
        Err(err) => {
            tracing::warn!(
                target: "orzma::clipboard",
                error = ?err,
                "clipboard write failed",
            );
        }
    }
}
