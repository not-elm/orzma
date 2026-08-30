//! Paste action pipeline: `on_paste` reads the system clipboard for a
//! `PasteAction` target and hands the text to the paste applier as
//! `PasteToTerminal`, which requests the paste on the underlying tty.

use crate::surface::OrzmaTerminal;
use bevy::{clipboard::ClipboardError, prelude::*};
use bevy_orzma_tty::prelude::RequestTtyPaste;

/// Pastes the system clipboard into the target terminal entity.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct PasteAction {
    /// The terminal entity to paste into.
    #[event_target]
    pub entity: Entity,
}

/// Registers the paste pipeline observers.
pub(super) struct ClipboardPasteActionPlugin;

impl Plugin for ClipboardPasteActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_paste)
            .add_observer(on_paste_to_terminal);
    }
}

/// Carries clipboard text to paste into a specific terminal entity. Emitted
/// by `on_paste` once the clipboard has been read, so the paste applier never
/// touches the clipboard resource and stays testable by triggering this event
/// directly.
#[derive(EntityEvent, Debug, Clone)]
struct PasteToTerminal {
    /// The terminal entity to paste into.
    #[event_target]
    terminal: Entity,
    /// The non-empty clipboard text to paste.
    text: String,
}

/// The decision `on_paste` derives from a clipboard read poll. Keeping the
/// branch logic a pure function of the poll result lets every arm be
/// unit-tested without a real clipboard backend (bevy's `Clipboard` exposes no
/// in-memory seam).
enum PasteRead {
    /// Non-empty clipboard text ready to paste.
    Ready(String),
    /// Nothing to paste — empty clipboard or a fetch not yet resolved.
    Nothing,
    /// No content or no clipboard backend (headless); logged at debug.
    Unavailable,
    /// The clipboard read failed; logged at warn with the error.
    Failed(ClipboardError),
}

impl PasteRead {
    fn classify(read: Option<Result<String, ClipboardError>>) -> Self {
        match read {
            Some(Ok(text)) if !text.is_empty() => Self::Ready(text),
            Some(Ok(_)) | None => Self::Nothing,
            // NOTE: ClipboardNotSupported (headless / no backend) is grouped
            // with ContentNotAvailable into the debug-logged Unavailable
            // outcome on purpose — a later edit must not let it fall through to
            // Failed. Failed logs at warn, so a clipboard-less host would then
            // emit a warning on every paste keystroke, masking real warnings.
            Some(Err(
                ClipboardError::ContentNotAvailable | ClipboardError::ClipboardNotSupported,
            )) => Self::Unavailable,
            Some(Err(err)) => Self::Failed(err),
        }
    }
}

fn on_paste(
    ev: On<PasteAction>,
    mut commands: Commands,
    mut clipboard: ResMut<Clipboard>,
    targets: Query<(), With<OrzmaTerminal>>,
) {
    if targets.get(ev.entity).is_err() {
        return;
    }
    match PasteRead::classify(clipboard.fetch_text().poll_result()) {
        PasteRead::Ready(text) => {
            commands.trigger(PasteToTerminal {
                terminal: ev.entity,
                text,
            });
        }
        PasteRead::Nothing => {}
        PasteRead::Unavailable => {
            tracing::debug!(
                target: "orzma::clipboard",
                "paste clipboard read: no content or no backend (empty / non-text / headless)",
            );
        }
        PasteRead::Failed(err) => {
            tracing::warn!(
                target: "orzma::clipboard",
                error = ?err,
                "paste clipboard read failed",
            );
        }
    }
}

/// Applies `PasteToTerminal` by requesting the paste on the underlying tty.
/// `OrzmaTty::send_paste` performs the bottom-snap / bracketed-paste framing
/// this observer used to do directly.
fn on_paste_to_terminal(ev: On<PasteToTerminal>, mut commands: Commands) {
    commands.trigger(RequestTtyPaste {
        terminal: ev.terminal,
        text: ev.text.clone(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_paste_ignores_non_terminal_entity() {
        #[derive(Resource, Default)]
        struct Emitted(usize);

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<Clipboard>()
            .init_resource::<Emitted>()
            .add_observer(on_paste)
            .add_observer(|_ev: On<PasteToTerminal>, mut emitted: ResMut<Emitted>| {
                emitted.0 += 1;
            });
        let entity = app.world_mut().spawn_empty().id();
        app.world_mut().trigger(PasteAction { entity });
        app.update();
        assert_eq!(
            app.world().resource::<Emitted>().0,
            0,
            "a PasteAction on a non-terminal entity must not read the clipboard or emit PasteToTerminal"
        );
    }

    #[test]
    fn classify_paste_read_ready_on_nonempty_text() {
        assert!(matches!(
            PasteRead::classify(Some(Ok("hi".to_string()))),
            PasteRead::Ready(text) if text == "hi"
        ));
    }

    #[test]
    fn classify_paste_read_nothing_on_empty_text() {
        assert!(matches!(
            PasteRead::classify(Some(Ok(String::new()))),
            PasteRead::Nothing
        ));
    }

    #[test]
    fn classify_paste_read_nothing_on_pending_fetch() {
        assert!(matches!(PasteRead::classify(None), PasteRead::Nothing));
    }

    #[test]
    fn classify_paste_read_unavailable_on_no_content_or_backend() {
        assert!(matches!(
            PasteRead::classify(Some(Err(ClipboardError::ContentNotAvailable))),
            PasteRead::Unavailable
        ));
        assert!(matches!(
            PasteRead::classify(Some(Err(ClipboardError::ClipboardNotSupported))),
            PasteRead::Unavailable
        ));
    }

    #[test]
    fn classify_paste_read_failed_on_other_error() {
        assert!(matches!(
            PasteRead::classify(Some(Err(ClipboardError::Unknown {
                description: "boom".to_string(),
            }))),
            PasteRead::Failed(_)
        ));
    }

    fn paste_applier_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_observer(on_paste_to_terminal);
        app
    }

    #[derive(Resource, Default)]
    struct SeenPastes(Vec<(Entity, String)>);

    /// Asserts that `PasteToTerminal` is forwarded as a `RequestTtyPaste`
    /// carrying the same target entity and text, unmodified.
    ///
    /// Case: `on_paste` already resolved a non-empty clipboard read and
    /// handed the applier the terminal + text to paste.
    #[test]
    fn paste_to_terminal_triggers_the_matching_request() {
        let mut app = paste_applier_app();
        app.init_resource::<SeenPastes>().add_observer(
            |ev: On<RequestTtyPaste>, mut seen: ResMut<SeenPastes>| {
                seen.0.push((ev.terminal, ev.text.clone()));
            },
        );
        let entity = app.world_mut().spawn_empty().id();

        app.world_mut().trigger(PasteToTerminal {
            terminal: entity,
            text: "hello".to_string(),
        });
        app.update();

        assert_eq!(
            app.world().resource::<SeenPastes>().0,
            vec![(entity, "hello".to_string())]
        );
    }
}
