//! Default-mode VI applier: applies the shared VI action events to the local
//! terminal engine (`TerminalHandle` vi/selection/scroll APIs). Guarded on
//! `Without<TmuxPane>` — tmux panes are handled by `vi/tmux_mode.rs`.

use crate::action::vi::{
    ViExitRequest, ViMotionRequest, ViScrollKind, ViScrollRequest, ViSelectionToggleRequest,
    ViYankRequest,
};
use crate::ui::copy_mode::ExitCopyMode;
use bevy::prelude::*;
use ozma_terminal::Clipboard;
use ozma_tty_engine::{Coalescer, SelectionType, TerminalHandle};
use ozmux_tmux::TmuxPane;

/// Registers the Default-mode VI apply observers. `ViPromptRequest` /
/// `ViSearchStepRequest` have no local applier yet (ignored by design).
pub(super) struct DefaultModeViPlugin;

impl Plugin for DefaultModeViPlugin {
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

type LocalTerminal<'w, 's> =
    Query<'w, 's, (&'static mut TerminalHandle, &'static mut Coalescer), Without<TmuxPane>>;

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

fn on_vi_yank(
    ev: On<ViYankRequest>,
    mut commands: Commands,
    mut clipboard: ResMut<Clipboard>,
    mut terminals: LocalTerminal,
) {
    let Ok((handle, _)) = terminals.get_mut(ev.entity) else {
        return;
    };
    if let Some(text) = handle.selection_to_string() {
        clipboard.write(text);
    }
    commands.trigger(ExitCopyMode { entity: ev.entity });
}

fn on_vi_exit(ev: On<ViExitRequest>, mut commands: Commands, terminals: LocalTerminal) {
    if terminals.get(ev.entity).is_err() {
        return;
    }
    commands.trigger(ExitCopyMode { entity: ev.entity });
}

/// Applies a scroll. Relative scrolls move the vi cursor with the viewport;
/// `Top`/`Bottom` snap to the buffer extremes.
fn apply_scroll(handle: &mut TerminalHandle, coalescer: &mut Coalescer, kind: ViScrollKind) {
    match kind {
        ViScrollKind::PageUp => handle.scroll_page_up(coalescer),
        ViScrollKind::PageDown => handle.scroll_page_down(coalescer),
        ViScrollKind::HalfUp => {
            let half = half_page(handle);
            handle.scroll(coalescer, half);
        }
        ViScrollKind::HalfDown => {
            let half = half_page(handle);
            handle.scroll(coalescer, -half);
        }
        ViScrollKind::LineUp => handle.scroll(coalescer, 1),
        ViScrollKind::LineDown => handle.scroll(coalescer, -1),
        ViScrollKind::Top => handle.scroll_to_top(coalescer),
        ViScrollKind::Bottom => handle.scroll_to_bottom(coalescer),
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
    fn appliers_ignore_tmux_panes_and_missing_entities() {
        use tmux_control_parser::{CellDims, PaneId};

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<Clipboard>();
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
        // A tmux pane must NOT be handled by the Default applier (Without<TmuxPane>).
        app.world_mut().trigger(ViExitRequest { entity: pane });
        app.world_mut().trigger(ViYankRequest { entity: pane });
        app.update();
    }
}
