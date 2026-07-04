//! Paste action: reads the system clipboard and writes it to the target
//! terminal entity's PTY as (optionally bracketed) paste bytes.

use crate::clipboard::{Clipboard, build_paste_bytes};
use crate::surface::OzmaTerminal;
use bevy::prelude::*;
use ozma_tty_engine::{Coalescer, PtyHandle, TerminalHandle};
use ozmux_tmux::TmuxPane;

/// Pastes the system clipboard into the target terminal entity's PTY.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct PasteAction {
    /// The terminal entity to paste into.
    #[event_target]
    pub entity: Entity,
}

/// Registers the paste apply observer.
pub(super) struct PastePlugin;

impl Plugin for PastePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_paste);
    }
}

fn on_paste(
    ev: On<PasteAction>,
    mut clipboard: ResMut<Clipboard>,
    // NOTE: Without<TmuxPane> is defensive, mirroring the IME-commit split
    // (`apply_ime_commit_to_terminal` in src/input/default_mode.rs) — tmux
    // panes never carry a PtyHandle (src/render/tmux.rs attaches them via
    // `TerminalHandle::detached`), so the `&mut PtyHandle` term below
    // already excludes them from this query. Keep the filter anyway so
    // this observer's tmux-exclusion doesn't rest solely on that
    // PtyHandle invariant; tmux paste is applied by `on_paste_tmux`
    // (src/action/tmux/paste.rs).
    mut terminals: Query<
        (&mut TerminalHandle, &mut PtyHandle, &mut Coalescer),
        (With<OzmaTerminal>, Without<TmuxPane>),
    >,
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
            .add_plugins(PastePlugin)
            .init_resource::<Clipboard>();
        let entity = app.world_mut().spawn_empty().id();
        app.world_mut().trigger(PasteAction { entity });
        app.update();
        // Reaching here proves the observer handled the missing-terminal and
        // unavailable/empty-clipboard paths without panicking. Byte correctness
        // is covered by the clipboard `build_paste_bytes_*` tests.
    }

    #[test]
    fn on_paste_is_noop_for_tmux_pane() {
        use ozmux_tmux::PaneId;
        use tmux_control_parser::CellDims;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(PastePlugin)
            .insert_resource(Clipboard::in_memory());
        app.world_mut()
            .resource_mut::<Clipboard>()
            .write("hello".to_string());

        let pane = app
            .world_mut()
            .spawn((
                OzmaTerminal,
                TmuxPane {
                    id: PaneId(1),
                    dims: CellDims {
                        width: 0,
                        height: 0,
                        xoff: 0,
                        yoff: 0,
                    },
                },
            ))
            .id();
        app.world_mut().trigger(PasteAction { entity: pane });
        app.update();
        // Reaching here proves the PTY-write path was not taken: the tmux
        // pane entity has no PtyHandle/Coalescer, so on_paste's query
        // cannot match it regardless of the Without<TmuxPane> filter.
    }
}
