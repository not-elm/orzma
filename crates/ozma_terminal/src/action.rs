//! PTY-level terminal actions as `EntityEvent`s. Each action is one event +
//! one observer, aggregated by `OzmaActionPlugin`. The first citizen is
//! `PasteAction`; mouse-driven scroll/copy actions join with the mouse module.

use crate::clipboard::{Clipboard, build_paste_bytes};
use crate::spawn::OzmaTerminal;
use bevy::prelude::*;
use ozma_tty_engine::{Coalescer, PtyHandle, TerminalHandle};

/// Pastes the system clipboard into the target terminal entity's PTY.
#[derive(EntityEvent, Debug, Clone)]
pub struct PasteAction {
    /// The terminal entity to paste into.
    #[event_target]
    pub entity: Entity,
}

/// Registers the crate's PTY-level action observers.
pub(crate) struct OzmaActionPlugin;

impl Plugin for OzmaActionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Clipboard>().add_observer(on_paste);
    }
}

fn on_paste(
    ev: On<PasteAction>,
    mut clipboard: ResMut<Clipboard>,
    mut terminals: Query<(&mut TerminalHandle, &mut PtyHandle, &mut Coalescer), With<OzmaTerminal>>,
) {
    let Some(text) = clipboard.read() else {
        return;
    };
    if text.is_empty() {
        return;
    }
    let Ok((mut handle, mut pty, mut coalescer)) = terminals.get_mut(ev.entity) else {
        return;
    };
    if !handle.is_at_bottom() {
        handle.scroll_to_bottom(&mut coalescer);
    }
    let bracketed = handle.bracketed_paste_enabled();
    let bytes = build_paste_bytes(&text, bracketed);
    if let Err(e) = handle.write(&mut pty, &bytes) {
        tracing::warn!(?e, entity = ?ev.entity, "ozma paste write failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paste_action_on_entity_without_terminal_does_not_panic() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(OzmaActionPlugin);
        let entity = app.world_mut().spawn_empty().id();
        app.world_mut().trigger(PasteAction { entity });
        app.update();
        // Reaching here proves the observer handled the missing-terminal and
        // unavailable/empty-clipboard paths without panicking. Byte correctness
        // is covered by the clipboard `build_paste_bytes_*` tests.
    }
}
