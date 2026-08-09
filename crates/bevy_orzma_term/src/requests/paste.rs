//! `RequestTermPaste`: text the host UI asks a terminal entity to receive
//! as if typed.

use bevy::prelude::*;

use crate::OrzmaTermHandle;

/// Fired by the host UI to paste text into a specific terminal entity.
///
/// Carries the clipboard text verbatim.
/// Bracketed-paste framing, marker stripping, and line-ending normalization all depend on terminal modes the
/// host cannot see, so they belong to the apply observer — the host reads the clipboard and nothing more.
#[derive(EntityEvent, Debug, Clone)]
pub struct RequestTermPaste {
    #[event_target]
    pub terminal: Entity,
    /// The text to paste, exactly as read from the clipboard.
    pub text: String,
}

/// Registers the [`RequestTermPaste`] apply observer.
pub(super) struct PastePlugin;

impl Plugin for PastePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_paste);
    }
}

fn apply_paste(e: On<RequestTermPaste>, mut terms: Query<&mut OrzmaTermHandle>) {
    if let Ok(mut tty) = terms.get_mut(e.terminal)
        && let Err(err) = tty.write_paste(&e.text)
    {
        error!(%err);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `(target, text)` an observer saw, in fire order.
    #[derive(Resource, Default)]
    struct Seen(Vec<(Entity, String)>);

    /// Observer that appends what it received to [`Seen`].
    fn record(ev: On<RequestTermPaste>, mut seen: ResMut<Seen>) {
        seen.0.push((ev.event_target(), ev.text.clone()));
    }

    fn paste(text: &str) -> (App, Entity) {
        let mut app = App::new();
        app.init_resource::<Seen>().add_observer(record);
        let terminal = app.world_mut().spawn_empty().id();
        app.world_mut().trigger(RequestTermPaste {
            terminal,
            text: text.to_owned(),
        });
        (app, terminal)
    }

    /// Asserts that the event delivers the clipboard text byte-identical —
    /// normal text, embedded paste markers, CR/LF, and empty alike.
    ///
    /// Case: the layer boundary. Bracketed-paste framing, marker stripping,
    /// and newline normalization are all decided by the terminal-mode-aware
    /// encoder below this event (`PtyInput::encode_paste`), because only that
    /// layer sees whether DECSET 2004 is active. A host or event layer that
    /// "helpfully" pre-sanitized would desync from the encoder's fixed-point
    /// stripping — so the contract this table pins is that raw text reaches
    /// the observer for mode-aware encoding, whatever it contains. It does
    /// NOT mean the markers are safe to forward as-is: stripping is the
    /// encoder's obligation, exercised by `orzma_term`'s paste tests.
    #[test]
    fn request_paste_preserves_raw_text_for_mode_aware_encoding() {
        for text in ["hello", "foo\x1b[201~rm -rf /\x1b[200~bar", "a\r\nb\nc", ""] {
            let (app, terminal) = paste(text);
            assert_eq!(
                app.world().resource::<Seen>().0,
                vec![(terminal, text.to_owned())],
                "text {text:?} must reach the observer byte-identical"
            );
        }
    }
}
