//! Forwards what an application does to the clipboard with OSC 52 to the
//! clipboard write seam.

use crate::action::clipboard::copy::{ClearClipboardAction, CopyAction};
use bevy::prelude::*;
use bevy_orzmux::prelude::{TtyClipboardClearSignal, TtyClipboardStoreSignal};

/// Adds the path from an application's OSC 52 writes and clears to the
/// clipboard write seam.
pub(super) struct ClipboardStoreActionPlugin;

impl Plugin for ClipboardStoreActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_clipboard_store)
            .add_observer(on_clipboard_clear);
    }
}

fn on_clipboard_store(ev: On<TtyClipboardStoreSignal>, mut commands: Commands) {
    commands.trigger(CopyAction {
        text: ev.content.clone(),
    });
}

fn on_clipboard_clear(_ev: On<TtyClipboardClearSignal>, mut commands: Commands) {
    commands.trigger(ClearClipboardAction);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts that an application's OSC 52 write reaches the clipboard
    /// copy seam carrying the same text.
    ///
    /// Case: a user yanks a line in Neovim over a remote session, and
    /// the editor's osc52 provider writes the yank to the PTY instead
    /// of reaching the host clipboard directly.
    #[test]
    fn an_application_clipboard_write_reaches_the_copy_seam() {
        #[derive(Resource, Default)]
        struct Copied(Vec<String>);

        let mut app = App::new();
        app.add_plugins((MinimalPlugins, ClipboardStoreActionPlugin))
            .init_resource::<Copied>()
            .add_observer(|ev: On<CopyAction>, mut copied: ResMut<Copied>| {
                copied.0.push(ev.text.clone());
            });
        let terminal = app.world_mut().spawn_empty().id();
        app.world_mut().trigger(TtyClipboardStoreSignal {
            terminal,
            content: "hi".to_owned(),
        });
        app.update();
        assert_eq!(app.world().resource::<Copied>().0, vec!["hi".to_owned()]);
    }

    /// Asserts that an application's OSC 52 clear reaches the clipboard
    /// write seam.
    ///
    /// Case: a program sends an OSC 52 whose payload is not base64.
    #[test]
    fn an_application_clipboard_clear_reaches_the_write_seam() {
        #[derive(Resource, Default)]
        struct Cleared(usize);

        let mut app = App::new();
        app.add_plugins((MinimalPlugins, ClipboardStoreActionPlugin))
            .init_resource::<Cleared>()
            .add_observer(
                |_ev: On<ClearClipboardAction>, mut cleared: ResMut<Cleared>| {
                    cleared.0 += 1;
                },
            );
        let terminal = app.world_mut().spawn_empty().id();
        app.world_mut()
            .trigger(TtyClipboardClearSignal { terminal });
        app.update();
        assert_eq!(app.world().resource::<Cleared>().0, 1);
    }
}
