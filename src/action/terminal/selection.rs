//! Selection actions on a terminal surface: clear the selection, copy it to
//! the clipboard, and write a copied text to the clipboard.

use crate::action::clipboard::CopyAction;
use crate::surface::OrzmaTerminal;
use bevy::prelude::*;
use bevy_orzmux::prelude::{
    RequestTtyCopySelection, RequestTtySelectionClear, TtySelectionTextSignal,
};

/// Clears any active local selection on `entity`.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct TerminalSelectionClear {
    /// The terminal entity whose selection is cleared.
    #[event_target]
    pub entity: Entity,
}

/// Copies `entity`'s current selection to the clipboard.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct TerminalSelectionCopy {
    /// The terminal entity whose selection is copied.
    #[event_target]
    pub entity: Entity,
    /// Whether the selection is dismissed once its text has been requested.
    /// When `false` the highlight survives the copy.
    pub dismiss: bool,
}

/// Triggers a `TerminalSelectionCopy` on the focused terminal, if any.
pub(crate) fn trigger_selection_copy(commands: &mut Commands, focused: Option<Entity>) {
    if let Some(entity) = focused {
        commands.trigger(TerminalSelectionCopy {
            entity,
            dismiss: true,
        });
    }
}

/// Adds the local-selection actions.
pub(super) struct SelectionPlugin;

impl Plugin for SelectionPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_terminal_selection_clear)
            .add_observer(on_terminal_selection_copy)
            .add_observer(on_selection_text);
    }
}

/// Applies a `TerminalSelectionClear` by requesting the same clear on the
/// underlying tty.
fn on_terminal_selection_clear(ev: On<TerminalSelectionClear>, mut commands: Commands) {
    commands.trigger(RequestTtySelectionClear {
        terminal: ev.entity,
    });
}

/// Applies a `TerminalSelectionCopy`: asks the backend for the pane's
/// selected text, and dismisses the selection when the event asks for it.
/// The answer arrives as `TtySelectionTextSignal`.
fn on_terminal_selection_copy(
    ev: On<TerminalSelectionCopy>,
    mut commands: Commands,
    terminals: Query<(), With<OrzmaTerminal>>,
) {
    if terminals.get(ev.entity).is_ok() {
        // NOTE: the copy request must be triggered before the clear. Both
        // become `OrzmuxCommand`s on one ordered channel, so the backend reads
        // the selected text only because it processes the copy first;
        // triggering the clear ahead of it would answer with empty text.
        commands.trigger(RequestTtyCopySelection {
            terminal: ev.entity,
        });
        if ev.dismiss {
            commands.trigger(TerminalSelectionClear { entity: ev.entity });
        }
    }
}

/// Writes an answered copy to the clipboard, skipping empty text so the
/// clipboard is never overwritten with nothing.
fn on_selection_text(ev: On<TtySelectionTextSignal>, mut commands: Commands) {
    if let Some(text) = ev.text.as_ref().filter(|t| !t.is_empty()) {
        commands.trigger(CopyAction { text: text.clone() });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Resource, Default)]
    struct SeenClears(Vec<Entity>);

    /// Asserts that `TerminalSelectionClear` is forwarded as a
    /// `RequestTtySelectionClear` targeting the same entity.
    ///
    /// Case: the user clicks elsewhere to dismiss an existing selection.
    #[test]
    fn selection_clear_triggers_the_matching_request() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<SeenClears>()
            .add_observer(on_terminal_selection_clear)
            .add_observer(
                |ev: On<RequestTtySelectionClear>, mut seen: ResMut<SeenClears>| {
                    seen.0.push(ev.terminal);
                },
            );
        let entity = app.world_mut().spawn_empty().id();

        app.world_mut().trigger(TerminalSelectionClear { entity });
        app.update();

        assert_eq!(app.world().resource::<SeenClears>().0, vec![entity]);
    }

    /// Asserts that a copy on a terminal entity asks the backend for its
    /// selection, and that an answered text becomes a clipboard write
    /// while an empty answer does not.
    ///
    /// Case: the user presses Cmd+C once with a selection and once with
    /// nothing selected.
    #[test]
    fn copy_asks_the_backend_and_writes_only_non_empty_answers() {
        #[derive(Resource, Default)]
        struct Seen {
            requests: Vec<Entity>,
            copies: Vec<String>,
        }
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(SelectionPlugin)
            .init_resource::<Seen>()
            .add_observer(|ev: On<RequestTtyCopySelection>, mut s: ResMut<Seen>| {
                s.requests.push(ev.terminal)
            })
            .add_observer(|ev: On<CopyAction>, mut s: ResMut<Seen>| s.copies.push(ev.text.clone()));
        let terminal = app.world_mut().spawn(OrzmaTerminal).id();
        app.world_mut().trigger(TerminalSelectionCopy {
            entity: terminal,
            dismiss: true,
        });
        app.world_mut().trigger(TtySelectionTextSignal {
            text: Some("hello".into()),
        });
        app.world_mut().trigger(TtySelectionTextSignal {
            text: Some(String::new()),
        });
        app.world_mut()
            .trigger(TtySelectionTextSignal { text: None });
        app.update();
        let seen = app.world().resource::<Seen>();
        assert_eq!(seen.requests, vec![terminal]);
        assert_eq!(seen.copies, vec!["hello".to_string()]);
    }

    /// Asserts that a dismissing copy clears the selection, and that it asks
    /// the backend for the text before clearing rather than after.
    ///
    /// Case: a Windows user presses `Ctrl+C` over a selection, then presses it
    /// again to interrupt the running command — which only reaches the shell
    /// because the first press left no selection behind.
    #[test]
    fn selection_copy_requests_text_then_clears_the_selection() {
        #[derive(Resource, Default)]
        struct Order(Vec<&'static str>);
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(SelectionPlugin)
            .init_resource::<Order>()
            .add_observer(|_: On<RequestTtyCopySelection>, mut o: ResMut<Order>| o.0.push("copy"))
            .add_observer(|_: On<RequestTtySelectionClear>, mut o: ResMut<Order>| {
                o.0.push("clear")
            });
        let terminal = app.world_mut().spawn(OrzmaTerminal).id();

        app.world_mut().trigger(TerminalSelectionCopy {
            entity: terminal,
            dismiss: true,
        });
        app.update();

        assert_eq!(app.world().resource::<Order>().0, vec!["copy", "clear"]);
    }

    /// Asserts that a non-dismissing copy asks for the text and leaves the
    /// selection in place.
    ///
    /// Case: the user finishes a drag selection and lets go of the left
    /// button, so orzma copies on release while the highlight stays up for a
    /// following `Ctrl+C`.
    #[test]
    fn selection_copy_without_dismiss_keeps_the_selection() {
        #[derive(Resource, Default)]
        struct Order(Vec<&'static str>);
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(SelectionPlugin)
            .init_resource::<Order>()
            .add_observer(|_: On<RequestTtyCopySelection>, mut o: ResMut<Order>| o.0.push("copy"))
            .add_observer(|_: On<RequestTtySelectionClear>, mut o: ResMut<Order>| {
                o.0.push("clear")
            });
        let terminal = app.world_mut().spawn(OrzmaTerminal).id();

        app.world_mut().trigger(TerminalSelectionCopy {
            entity: terminal,
            dismiss: false,
        });
        app.update();

        assert_eq!(app.world().resource::<Order>().0, vec!["copy"]);
    }

    /// Asserts that a copy aimed at an entity without a terminal clears
    /// nothing, so a stale event cannot dismiss another pane's selection.
    ///
    /// Case: a copy event in flight while its target pane is torn down.
    #[test]
    fn selection_copy_on_a_bare_entity_clears_nothing() {
        #[derive(Resource, Default)]
        struct Order(Vec<&'static str>);
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(SelectionPlugin)
            .init_resource::<Order>()
            .add_observer(|_: On<RequestTtyCopySelection>, mut o: ResMut<Order>| o.0.push("copy"))
            .add_observer(|_: On<RequestTtySelectionClear>, mut o: ResMut<Order>| {
                o.0.push("clear")
            });
        let bare = app.world_mut().spawn_empty().id();

        app.world_mut().trigger(TerminalSelectionCopy {
            entity: bare,
            dismiss: true,
        });
        app.update();

        assert!(app.world().resource::<Order>().0.is_empty());
    }
}
