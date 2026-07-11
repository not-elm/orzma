//! Default-mode paste applier: writes `PasteToTerminal` text into the target
//! terminal's PTY as (optionally bracketed) paste bytes.

use super::{PasteToTerminal, build_paste_bytes};
use crate::surface::OrzmaTerminal;
use bevy::prelude::*;
use orzma_tmux::TmuxPane;
use orzma_tty_engine::{Coalescer, PtyHandle, TerminalHandle};

/// Registers the Default-mode paste applier.
pub(super) struct PasteDefaultModePlugin;

impl Plugin for PasteDefaultModePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_paste_default_mode);
    }
}

/// Applies `PasteToTerminal` to a PTY-attached terminal: snaps a scrolled-back
/// viewport to the bottom, then writes the (optionally bracketed) paste
/// bytes. The query filters select only PTY-attached terminals.
fn on_paste_default_mode(
    ev: On<PasteToTerminal>,
    mut terminals: Query<
        (&mut TerminalHandle, &mut PtyHandle, &mut Coalescer),
        (With<OrzmaTerminal>, Without<TmuxPane>),
    >,
) {
    let Ok((mut handle, mut pty, mut coalescer)) = terminals.get_mut(ev.terminal) else {
        return;
    };
    if !handle.is_at_bottom() {
        handle.scroll_to_bottom(&mut coalescer);
    }
    let bracketed = handle.bracketed_paste_enabled();
    let bytes = build_paste_bytes(&ev.text, bracketed);
    if let Err(e) = handle.write(&mut pty, &bytes) {
        tracing::warn!(?e, entity = ?ev.terminal, "orzma paste write failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_mode_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(PasteDefaultModePlugin);
        app
    }

    #[test]
    fn on_paste_without_terminal_does_not_panic() {
        let mut app = default_mode_app();
        let entity = app.world_mut().spawn_empty().id();
        app.world_mut().trigger(PasteToTerminal {
            terminal: entity,
            text: "hello".to_string(),
        });
        app.update();
        // Reaching here proves the missing-terminal path did not panic. Byte
        // correctness is covered by the `build_paste_bytes_*` tests.
    }

    #[test]
    fn on_paste_is_noop_for_tmux_pane() {
        use orzma_tmux::PaneId;
        use tmux_control_parser::CellDims;

        let mut app = default_mode_app();
        let pane = app
            .world_mut()
            .spawn((
                OrzmaTerminal,
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
        app.world_mut().trigger(PasteToTerminal {
            terminal: pane,
            text: "hello".to_string(),
        });
        app.update();
        // Reaching here proves the PTY-write path was not taken: the tmux
        // pane entity has no PtyHandle/Coalescer, so the query cannot match
        // it regardless of the Without<TmuxPane> filter.
    }
}
