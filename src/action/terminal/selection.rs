//! Local-selection actions: start / update / clear a selection on a terminal
//! surface, and copy the current selection to the clipboard.

use crate::action::terminal::apply_to_terminal;
use bevy::prelude::*;
use ozma_terminal::{Clipboard, OzmaTerminal};
use ozma_tty_engine::{Coalescer, Point, PtyHandle, SelectionType, Side, TerminalHandle};

/// Starts a new local selection on `entity` at `point`.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct TerminalSelectionStart {
    /// The terminal entity to start the selection on.
    #[event_target]
    pub entity: Entity,
    /// The viewport-relative anchor of the new selection.
    pub point: Point,
    /// Which half of the cell the anchor sits in.
    pub side: Side,
    /// The selection granularity (simple / semantic / lines).
    pub ty: SelectionType,
}

/// Extends `entity`'s current selection's moving end to `point`.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct TerminalSelectionUpdate {
    /// The terminal entity whose selection is extended.
    #[event_target]
    pub entity: Entity,
    /// The viewport-relative moving end.
    pub point: Point,
    /// Which half of the cell the moving end sits in.
    pub side: Side,
}

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
}

/// Registers the selection apply observers.
pub(super) struct SelectionPlugin;

impl Plugin for SelectionPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_terminal_selection_start)
            .add_observer(on_terminal_selection_update)
            .add_observer(on_terminal_selection_clear)
            .add_observer(on_terminal_selection_copy);
    }
}

/// Applies a `TerminalSelectionStart`.
fn on_terminal_selection_start(
    ev: On<TerminalSelectionStart>,
    mut commands: Commands,
    mut terminals: Query<
        (
            &mut TerminalHandle,
            Option<&mut PtyHandle>,
            Option<&mut Coalescer>,
        ),
        With<OzmaTerminal>,
    >,
) {
    let Ok((mut handle, pty, coalescer)) = terminals.get_mut(ev.entity) else {
        return;
    };
    apply_to_terminal(
        &mut commands,
        &mut handle,
        pty,
        coalescer,
        ev.entity,
        |handle, _pty, coalescer| handle.selection_start_at(coalescer, ev.point, ev.side, ev.ty),
        |_commands, handle, _entity| {
            handle.selection_start_at_vt_only(ev.point, ev.side, ev.ty);
            true
        },
    );
}

/// Applies a `TerminalSelectionUpdate`.
fn on_terminal_selection_update(
    ev: On<TerminalSelectionUpdate>,
    mut commands: Commands,
    mut terminals: Query<
        (
            &mut TerminalHandle,
            Option<&mut PtyHandle>,
            Option<&mut Coalescer>,
        ),
        With<OzmaTerminal>,
    >,
) {
    let Ok((mut handle, pty, coalescer)) = terminals.get_mut(ev.entity) else {
        return;
    };
    apply_to_terminal(
        &mut commands,
        &mut handle,
        pty,
        coalescer,
        ev.entity,
        |handle, _pty, coalescer| handle.selection_update_to(coalescer, ev.point, ev.side),
        |_commands, handle, _entity| {
            handle.selection_update_to_vt_only(ev.point, ev.side);
            true
        },
    );
}

/// Applies a `TerminalSelectionClear`.
fn on_terminal_selection_clear(
    ev: On<TerminalSelectionClear>,
    mut commands: Commands,
    mut terminals: Query<
        (
            &mut TerminalHandle,
            Option<&mut PtyHandle>,
            Option<&mut Coalescer>,
        ),
        With<OzmaTerminal>,
    >,
) {
    let Ok((mut handle, pty, coalescer)) = terminals.get_mut(ev.entity) else {
        return;
    };
    apply_to_terminal(
        &mut commands,
        &mut handle,
        pty,
        coalescer,
        ev.entity,
        |handle, _pty, coalescer| handle.selection_clear(coalescer),
        |_commands, handle, _entity| {
            handle.selection_clear_vt_only();
            true
        },
    );
}

/// Applies a `TerminalSelectionCopy`: writes the selection text (if any) to
/// the clipboard. Needs only read access to the handle.
fn on_terminal_selection_copy(
    ev: On<TerminalSelectionCopy>,
    mut clipboard: ResMut<Clipboard>,
    terminals: Query<&TerminalHandle, With<OzmaTerminal>>,
) {
    let Ok(handle) = terminals.get(ev.entity) else {
        return;
    };
    if let Some(text) = handle.selection_to_string() {
        clipboard.write(text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ozma_tty_engine::{Column, Line};

    #[test]
    fn detached_selection_start_event_sets_selection_and_emits_frame() {
        use ozma_tty_engine::TerminalHandle;
        use ozma_tty_renderer::schema::{FrameDelta, FrameSnapshot};

        #[derive(Resource, Default)]
        struct FramesEmitted(usize);

        let mut app = App::new();
        app.init_resource::<Clipboard>()
            .init_resource::<FramesEmitted>()
            .add_observer(on_terminal_selection_start)
            .add_observer(
                |_ev: On<FrameSnapshot>, mut emitted: ResMut<FramesEmitted>| {
                    emitted.0 += 1;
                },
            )
            .add_observer(|_ev: On<FrameDelta>, mut emitted: ResMut<FramesEmitted>| {
                emitted.0 += 1;
            });

        let handle = TerminalHandle::detached(10, 5);
        let entity = app.world_mut().spawn((OzmaTerminal, handle)).id();

        app.world_mut().trigger(TerminalSelectionStart {
            entity,
            point: Point::new(Line(0), Column(0)),
            side: Side::Left,
            ty: SelectionType::Simple,
        });
        app.world_mut().flush();

        let handle = app.world().entity(entity).get::<TerminalHandle>().unwrap();
        assert!(
            handle.selection_to_string().is_some(),
            "TerminalSelectionStart on a PTY-less OzmaTerminal must set a selection via vt_only"
        );
        assert!(
            app.world().resource::<FramesEmitted>().0 >= 1,
            "the detached selection apply must flush_emit a frame so the renderer repaints"
        );
    }
}
