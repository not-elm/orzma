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
    if let Ok(mut tty) = terms.get_mut(e.terminal) {
        // tty.paste(e.text);
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

    /// Asserts that a triggered `RequestTermPaste` reaches an observer with
    /// its target and text intact.
    ///
    /// Case: the ordinary Cmd/Ctrl-V path — the host has resolved the
    /// clipboard read and hands the resulting string to the focused terminal.
    #[test]
    fn trigger_delivers_the_clipboard_text() {
        let (app, terminal) = paste("hello");
        assert_eq!(
            app.world().resource::<Seen>().0,
            vec![(terminal, "hello".to_owned())]
        );
    }

    /// Asserts that embedded bracketed-paste markers reach the observer
    /// unstripped.
    ///
    /// Case: the kitty/Alacritty paste-injection class (kitty commit 668f6fa,
    /// Alacritty issue #800) — hostile clipboard content carrying `ESC[201~`
    /// tries to close the paste bracket early and have the rest run as typed
    /// input. Stripping is the apply observer's job because only it knows
    /// whether bracketed paste is even active; this test pins that the event
    /// does not half-sanitize on the way and leave the observer believing the
    /// text is already safe.
    #[test]
    fn embedded_paste_markers_are_not_stripped_in_transit() {
        let hostile = "foo\x1b[201~rm -rf /\x1b[200~bar";
        let (app, terminal) = paste(hostile);
        assert_eq!(
            app.world().resource::<Seen>().0,
            vec![(terminal, hostile.to_owned())],
            "sanitization belongs to the apply observer, not to the event"
        );
    }

    /// Asserts that line endings and empty text pass through untouched.
    ///
    /// Case: a multi-line paste from an editor arrives as `\r\n`, while shells
    /// expect one `\r` per line — a conversion the apply observer performs.
    /// Empty text is included because the host can legitimately fire on an
    /// empty clipboard, and dropping it here would hide that from the observer.
    #[test]
    fn line_endings_and_empty_text_pass_through() {
        let (app, terminal) = paste("a\r\nb\nc");
        assert_eq!(
            app.world().resource::<Seen>().0,
            vec![(terminal, "a\r\nb\nc".to_owned())]
        );

        let (app, terminal) = paste("");
        assert_eq!(
            app.world().resource::<Seen>().0,
            vec![(terminal, String::new())]
        );
    }
}
