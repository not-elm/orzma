//! Copy action: the single clipboard write seam. Copy / yank observers
//! trigger `CopyAction`; `on_copy` performs the one `Clipboard::set_text`.

use bevy::{clipboard::ClipboardError, prelude::*};

/// Requests that `text` be written to the system clipboard.
#[derive(Event, Debug, Clone)]
pub(crate) struct CopyAction {
    /// The text to place on the system clipboard.
    pub text: String,
}

/// Adds orzma's clipboard write path onto Bevy's `Clipboard` resource,
/// provided by `DefaultPlugins`.
pub(super) struct ClipboardCopyActionPlugin;

impl Plugin for ClipboardCopyActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_copy);
    }
}

fn on_copy(ev: On<CopyAction>, mut clipboard: ResMut<Clipboard>) {
    match clipboard.set_text(ev.text.as_str()) {
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
