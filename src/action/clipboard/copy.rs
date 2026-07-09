use bevy::{clipboard::ClipboardError, prelude::*};

#[derive(Debug, Event)]
pub struct CopyAction {
    pub text: String,
}

pub struct ClipboardCopyActionPlugin;

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
