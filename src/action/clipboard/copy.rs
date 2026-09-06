//! Copy action: the single clipboard write seam. Copy / yank observers
//! trigger `CopyAction`; `on_copy` performs the one `Clipboard::set_text`.

use bevy::{clipboard::ClipboardError, prelude::*};

/// Requests that `text` be written to the system clipboard.
///
/// A global (non-entity) event: the clipboard is process-global, so copy /
/// yank observers `commands.trigger` this and `on_copy` performs the single
/// `Clipboard::set_text`. Decoupling this way keeps the copy observers
/// testable by capturing the request instead of round-tripping a real
/// clipboard.
#[derive(Event, Debug, Clone)]
pub(crate) struct CopyAction {
    /// The text to place on the system clipboard.
    pub text: String,
}

/// Registers the clipboard write-seam observer. Bevy's `Clipboard` resource
/// comes from `DefaultPlugins`; this plugin only adds orzma's write path.
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
