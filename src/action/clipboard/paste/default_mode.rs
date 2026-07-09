//! Default-mode paste applier: writes `PasteToTerminal` text into the target
//! terminal's PTY as (optionally bracketed) paste bytes. Gated on
//! `AppMode::Default` at registration.

use super::{PasteToTerminal, build_paste_bytes};
use crate::app_mode::AppMode;
use crate::surface::OrzmaTerminal;
use bevy::prelude::*;
use orzma_tmux::TmuxPane;
use orzma_tty_engine::{Coalescer, PtyHandle, TerminalHandle};

/// Registers the Default-mode paste applier, gated on `AppMode::Default`.
pub(super) struct PasteDefaultModePlugin;

impl Plugin for PasteDefaultModePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_paste_default_mode.run_if(in_state(AppMode::Default)));
    }
}

/// Applies `PasteToTerminal` to a PTY-attached terminal: snaps a scrolled-back
/// viewport to the bottom, then writes the (optionally bracketed) paste bytes.
/// The registration `run_if` is the authoritative mode routing; the query
/// filters stay as defense in depth.
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
    use bevy::state::app::StatesPlugin;

    fn default_mode_app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin));
        app.insert_state(AppMode::Default);
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

    #[test]
    fn paste_to_terminal_in_tmux_mode_does_not_run_default_applier() {
        use bevy::ecs::system::RunSystemOnce;
        use orzma_tty_engine::{SpawnOptions, TerminalBundle};

        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin));
        app.insert_state(AppMode::Tmux);
        app.add_plugins(PasteDefaultModePlugin);

        let opts = SpawnOptions {
            cols: 10,
            rows: 5,
            shell: "/bin/sh".into(),
            cwd: None,
            env: Vec::new(),
        };
        let bundle = TerminalBundle::spawn(opts).expect("spawn /bin/sh");
        let entity = app.world_mut().spawn((OrzmaTerminal, bundle)).id();

        app.world_mut()
            .run_system_once(
                move |mut terminals: Query<(&mut TerminalHandle, &mut Coalescer)>| {
                    let (mut handle, _) = terminals.get_mut(entity).unwrap();
                    for _ in 0..20 {
                        handle.advance(b"line\r\n");
                    }
                    handle.scroll_vt_only(1);
                    assert!(
                        !handle.is_at_bottom(),
                        "fixture precondition: terminal must start scrolled back"
                    );
                },
            )
            .unwrap();

        app.world_mut().trigger(PasteToTerminal {
            terminal: entity,
            text: "hello".to_string(),
        });
        app.update();

        let handle = app.world().get::<TerminalHandle>(entity).unwrap();
        assert!(
            !handle.is_at_bottom(),
            "the default paste applier is run_if-gated on AppMode::Default and must not snap the viewport in Tmux mode"
        );
    }
}
