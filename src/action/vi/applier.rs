//! Local VI applier: applies the shared VI action events to the local
//! terminal engine (`TerminalHandle` vi/selection/scroll APIs) for every
//! pane, tmux and non-tmux alike.

use crate::action::vi::{
    ViExitRequest, ViMotionRequest, ViScrollRequest, ViSelectionToggleRequest, ViYankRequest,
};
use crate::clipboard::ClipboardWriteRequest;
use crate::ui::vi_mode::ExitViMode;
use bevy::prelude::*;
use orzma_configs::vi_mode::ViModeScroll;
use orzma_tty_engine::{Coalescer, SelectionType, TerminalHandle};

/// Registers the local VI apply observers. `ViPromptRequest` /
/// `ViSearchStepRequest` have no local applier yet (ignored by design).
pub(super) struct ViApplierPlugin;

impl Plugin for ViApplierPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_vi_motion)
            .add_observer(on_vi_scroll)
            .add_observer(on_vi_selection_toggle)
            .add_observer(on_vi_yank)
            .add_observer(on_vi_exit);
    }
}

/// A resolved selection-toggle operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectionOp {
    Start(SelectionType),
    Change(SelectionType),
    Clear,
}

/// Resolves a selection toggle against the current selection: same kind
/// clears, a different kind switches, none starts.
fn resolve_selection_toggle(
    current: Option<SelectionType>,
    requested: SelectionType,
) -> SelectionOp {
    match current {
        Some(c) if c == requested => SelectionOp::Clear,
        Some(_) => SelectionOp::Change(requested),
        None => SelectionOp::Start(requested),
    }
}

type LocalTerminal<'w, 's> = Query<'w, 's, (&'static mut TerminalHandle, &'static mut Coalescer)>;

fn on_vi_motion(ev: On<ViMotionRequest>, mut terminals: LocalTerminal) {
    let Ok((mut handle, mut coalescer)) = terminals.get_mut(ev.entity) else {
        return;
    };
    handle.vi_motion(&mut coalescer, ev.motion);
}

fn on_vi_scroll(ev: On<ViScrollRequest>, mut terminals: LocalTerminal) {
    let Ok((mut handle, mut coalescer)) = terminals.get_mut(ev.entity) else {
        return;
    };
    apply_scroll(&mut handle, &mut coalescer, ev.kind);
}

fn on_vi_selection_toggle(ev: On<ViSelectionToggleRequest>, mut terminals: LocalTerminal) {
    let Ok((mut handle, mut coalescer)) = terminals.get_mut(ev.entity) else {
        return;
    };
    let op = resolve_selection_toggle(handle.selection_type(), ev.ty);
    match op {
        SelectionOp::Start(ty) => handle.selection_start(&mut coalescer, ty),
        SelectionOp::Change(ty) => {
            if !handle.selection_change_type(&mut coalescer, ty) {
                handle.selection_start(&mut coalescer, ty);
            }
        }
        SelectionOp::Clear => handle.selection_clear(&mut coalescer),
    }
}

fn on_vi_yank(ev: On<ViYankRequest>, mut commands: Commands, mut terminals: LocalTerminal) {
    let Ok((handle, _)) = terminals.get_mut(ev.entity) else {
        return;
    };
    if let Some(text) = handle.selection_to_string() {
        commands.trigger(ClipboardWriteRequest { text });
    }
    commands.trigger(ExitViMode { entity: ev.entity });
}

fn on_vi_exit(ev: On<ViExitRequest>, mut commands: Commands, terminals: LocalTerminal) {
    if terminals.get(ev.entity).is_err() {
        return;
    }
    commands.trigger(ExitViMode { entity: ev.entity });
}

/// Applies a scroll. Relative scrolls move the vi cursor with the viewport;
/// `Top`/`Bottom` snap to the buffer extremes.
fn apply_scroll(handle: &mut TerminalHandle, coalescer: &mut Coalescer, kind: ViModeScroll) {
    match kind {
        ViModeScroll::PageUp => handle.scroll_page_up(coalescer),
        ViModeScroll::PageDown => handle.scroll_page_down(coalescer),
        ViModeScroll::HalfPageUp => {
            let half = half_page(handle);
            handle.scroll(coalescer, half);
        }
        ViModeScroll::HalfPageDown => {
            let half = half_page(handle);
            handle.scroll(coalescer, -half);
        }
        ViModeScroll::ScrollUp => handle.scroll(coalescer, 1),
        ViModeScroll::ScrollDown => handle.scroll(coalescer, -1),
        ViModeScroll::HistoryTop => handle.scroll_to_top(coalescer),
        ViModeScroll::HistoryBottom => handle.scroll_to_bottom(coalescer),
    }
}

/// Half the visible row count (at least 1).
fn half_page(handle: &TerminalHandle) -> i32 {
    (handle.read_geometry().1 as i32 / 2).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_toggle_resolution() {
        assert_eq!(
            resolve_selection_toggle(None, SelectionType::Simple),
            SelectionOp::Start(SelectionType::Simple)
        );
        assert_eq!(
            resolve_selection_toggle(Some(SelectionType::Simple), SelectionType::Simple),
            SelectionOp::Clear
        );
        assert_eq!(
            resolve_selection_toggle(Some(SelectionType::Simple), SelectionType::Lines),
            SelectionOp::Change(SelectionType::Lines)
        );
    }

    #[test]
    fn appliers_ignore_entities_missing_coalescer() {
        use orzma_tmux::TmuxPane;
        use tmux_control_parser::{CellDims, PaneId};

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_observer(on_vi_exit);
        app.add_observer(on_vi_yank);
        let pane = app
            .world_mut()
            .spawn((
                TmuxPane {
                    id: PaneId(1),
                    dims: CellDims {
                        width: 10,
                        height: 5,
                        xoff: 0,
                        yoff: 0,
                    },
                },
                TerminalHandle::detached(10, 5),
            ))
            .id();
        // No Coalescer on this entity, so the query does not match — no panic.
        app.world_mut().trigger(ViExitRequest { entity: pane });
        app.world_mut().trigger(ViYankRequest { entity: pane });
        app.update();
    }

    #[test]
    fn vi_scroll_applies_to_a_tmux_pane_entity() {
        use orzma_configs::vi_mode::ViModeScroll;
        use orzma_tmux::TmuxPane;
        use orzma_tty_engine::{Coalescer, TerminalHandle};
        use tmux_control_parser::{CellDims, PaneId};

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_observer(on_vi_scroll);
        let mut handle = TerminalHandle::detached(20, 5);
        handle.advance(b"l1\r\nl2\r\nl3\r\nl4\r\nl5\r\nl6\r\nl7\r\nl8\r\nl9\r\nl10\r\n");
        let entity = app
            .world_mut()
            .spawn((
                handle,
                Coalescer::default(),
                TmuxPane {
                    id: PaneId(1),
                    dims: CellDims {
                        width: 20,
                        height: 5,
                        xoff: 0,
                        yoff: 0,
                    },
                },
            ))
            .id();
        app.world_mut().trigger(ViScrollRequest {
            entity,
            kind: ViModeScroll::ScrollUp,
        });
        let snapshot = app
            .world()
            .get::<TerminalHandle>(entity)
            .unwrap()
            .vi_indicator_snapshot();
        // Before this task the LocalTerminal query is Without<TmuxPane>, so the
        // observer no-ops and scroll_offset stays 0 — this assertion only holds
        // once the guard is removed and `handle.scroll(...)` actually runs.
        assert!(
            snapshot.scroll_offset > 0,
            "applier did not run on a TmuxPane entity"
        );
    }

    #[test]
    fn yank_writes_selection_to_clipboard_and_exits() {
        use crate::ui::vi_mode::{EnterViModeActionEvent, ViModePlugin, ViModeState};
        use bevy::ecs::system::RunSystemOnce;
        use orzma_tty_engine::{SpawnOptions, TerminalBundle, ViMotion};

        #[derive(Resource, Default)]
        struct CapturedCopies(Vec<String>);

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(ViModePlugin);
        app.add_observer(on_vi_yank);
        // Capture the write-seam request instead of round-tripping a real
        // clipboard: headless-safe and never clobbers the developer's clipboard.
        app.init_resource::<CapturedCopies>();
        app.add_observer(
            |ev: On<ClipboardWriteRequest>, mut captured: ResMut<CapturedCopies>| {
                captured.0.push(ev.text.clone());
            },
        );

        let opts = SpawnOptions {
            cols: 20,
            rows: 5,
            shell: "/bin/sh".into(),
            cwd: None,
            env: Vec::new(),
        };
        let bundle = TerminalBundle::spawn(opts).expect("spawn /bin/sh");
        let entity = app.world_mut().spawn(bundle).id();

        app.world_mut().trigger(EnterViModeActionEvent { entity });
        app.update();
        app.world_mut()
            .run_system_once(move |mut q: Query<(&mut TerminalHandle, &mut Coalescer)>| {
                let (mut h, mut c) = q.get_mut(entity).unwrap();
                h.advance(b"hello world");
                h.selection_start(&mut c, SelectionType::Simple);
                h.vi_motion(&mut c, ViMotion::Last);
            })
            .unwrap();

        app.world_mut().trigger(ViYankRequest { entity });
        app.update();

        assert!(app.world().get::<ViModeState>(entity).is_none());
        let captured = &app.world().resource::<CapturedCopies>().0;
        assert!(
            captured.iter().any(|t| !t.is_empty()),
            "yank must emit a non-empty ClipboardWriteRequest"
        );
    }
}
