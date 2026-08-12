//! `RequestTermSelection`: the selection operation the host UI asks a
//! terminal entity to perform.
//!
//! The selection vocabulary ([`SelectionOp`], [`SelectionKind`],
//! [`CellSide`]) is owned by the VT layer; this module re-exports it so
//! the request and its payload types travel together — the request
//! carries exactly what the VT applies.

use bevy::prelude::*;
pub use orzma_vt::prelude::{CellSide, SelectionKind, SelectionOp};

/// Fired by the host UI to change a specific terminal entity's selection.
///
/// The observer's only job is routing the operation to the targeted
/// entity's handle. Anchor resolution, cell-side inclusion, and geometry
/// live in `orzma_vt` and are pinned by its tests, not re-asserted here.
#[derive(EntityEvent, Debug, Clone)]
pub struct RequestTermSelection {
    #[event_target]
    pub terminal: Entity,
    /// The operation to perform.
    pub op: SelectionOp,
}

pub(super) struct SelectionPlugin;

impl Plugin for SelectionPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_selection);
    }
}

fn apply_selection(e: On<RequestTermSelection>) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OrzmaTermHandle;
    use orzma_vt::prelude::{SelectionRange, ViewportPoint, VtBackend};

    fn app_with_terminal(seed: &[u8]) -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins(SelectionPlugin);
        let (mut handle, _) = OrzmaTermHandle::detached(80, 24);
        handle.vt_mut().interpret(seed);
        let terminal = app.world_mut().spawn(handle).id();
        (app, terminal)
    }

    fn cell(x: u16, y: i16) -> ViewportPoint {
        ViewportPoint { row: y, column: x }
    }

    fn trigger_op(app: &mut App, terminal: Entity, op: SelectionOp) {
        app.world_mut()
            .trigger(RequestTermSelection { terminal, op });
    }

    fn start_simple(app: &mut App, terminal: Entity, x: u16, y: i16) {
        trigger_op(
            app,
            terminal,
            SelectionOp::StartAt {
                cell: cell(x, y),
                side: CellSide::Left,
                kind: SelectionKind::Simple,
            },
        );
    }

    fn update_to(app: &mut App, terminal: Entity, x: u16, y: i16, side: CellSide) {
        trigger_op(
            app,
            terminal,
            SelectionOp::UpdateTo {
                cell: cell(x, y),
                side,
            },
        );
    }

    fn selection_range(app: &mut App, terminal: Entity) -> Option<SelectionRange> {
        app.world_mut()
            .get_mut::<OrzmaTermHandle>(terminal)
            .expect("terminal entity must keep its handle")
            .vt_mut()
            .selection_range()
    }

    fn selected_text(app: &mut App, terminal: Entity) -> Option<String> {
        app.world_mut()
            .get_mut::<OrzmaTermHandle>(terminal)
            .expect("terminal entity must keep its handle")
            .vt_mut()
            .selected_text()
    }

    /// Asserts that press and drag requests select the dragged span in
    /// the targeted entity's VT.
    ///
    /// Case: the mouse path, where a press anchors the selection, a
    /// drag extends it, and a copy reads back the span. An observer
    /// that drops or mangles the forwarded operation paints either no
    /// highlight or the wrong one. Cell-side inclusion, granularity,
    /// and geometry are the VT's job and are pinned in `orzma_vt`.
    #[test]
    fn a_press_and_drag_select_the_dragged_span() {
        let (mut app, terminal) = app_with_terminal(b"abcdefghij");
        start_simple(&mut app, terminal, 0, 0);
        update_to(&mut app, terminal, 4, 0, CellSide::Right);
        assert_eq!(selected_text(&mut app, terminal).as_deref(), Some("abcde"));
    }

    /// Asserts that a `Clear` request drops the active selection.
    ///
    /// Case: the user clicks elsewhere to dismiss an existing
    /// selection. A forward that covers only the start and update
    /// operations leaves a stale highlight that the renderer keeps
    /// painting after the user has cleared it.
    #[test]
    fn clear_drops_the_active_selection() {
        let (mut app, terminal) = app_with_terminal(b"abcdefghij");
        start_simple(&mut app, terminal, 0, 0);
        update_to(&mut app, terminal, 4, 0, CellSide::Right);
        assert!(
            selection_range(&mut app, terminal).is_some(),
            "precondition: the drag must have selected something"
        );
        trigger_op(&mut app, terminal, SelectionOp::Clear);
        assert_eq!(selection_range(&mut app, terminal), None);
    }
}
