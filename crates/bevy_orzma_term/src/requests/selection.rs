//! Per-operation selection request events the host UI fires at a
//! terminal entity.
//!
//! The payload vocabulary ([`SelectionKind`], [`CellSide`],
//! [`ViewportPoint`]) is owned by the VT layer; this module re-exports
//! it so the requests and their payload types travel together — each
//! request carries exactly what the VT applies.

use bevy::prelude::*;
pub use orzma_vt::prelude::{CellSide, SelectionKind, ViewportPoint};

use crate::OrzmaTermHandle;

/// Fired by the host UI to anchor a new selection at an explicit
/// viewport cell (mouse press).
///
/// The observers in this module only route operations to the targeted
/// entity's handle. Anchor resolution, cell-side inclusion, and
/// geometry live in `orzma_vt` and are pinned by its tests, not
/// re-asserted here.
#[derive(EntityEvent, Debug, Clone)]
pub struct RequestTermSelectionStart {
    #[event_target]
    pub terminal: Entity,
    /// Viewport cell the press landed on.
    pub cell: ViewportPoint,
    /// Which half of the cell the anchor sits in.
    pub side: CellSide,
    /// Granularity of the new selection.
    pub kind: SelectionKind,
}

/// Fired by the host UI to anchor a new selection at the vi cursor
/// (vi-mode `v` / `V`), whose position only the VT knows.
#[derive(EntityEvent, Debug, Clone)]
pub struct RequestTermSelectionStartAtViCursor {
    #[event_target]
    pub terminal: Entity,
    /// Granularity of the new selection.
    pub kind: SelectionKind,
}

/// Fired by the host UI to move the moving end of the active selection
/// (mouse drag).
#[derive(EntityEvent, Debug, Clone)]
pub struct RequestTermSelectionUpdate {
    #[event_target]
    pub terminal: Entity,
    /// Viewport cell the moving end is dragged to. May sit outside the
    /// viewport when the drag leaves it.
    pub cell: ViewportPoint,
    /// Which half of the cell the moving end sits in.
    pub side: CellSide,
}

/// Fired by the host UI to switch selection granularity while keeping
/// the anchor (vi-mode `v` while `V` is active, and the reverse).
#[derive(EntityEvent, Debug, Clone)]
pub struct RequestTermSelectionKindChange {
    #[event_target]
    pub terminal: Entity,
    /// The granularity to switch to.
    pub kind: SelectionKind,
}

/// Fired by the host UI to drop any active selection.
#[derive(EntityEvent, Debug, Clone)]
pub struct RequestTermSelectionClear {
    #[event_target]
    pub terminal: Entity,
}

pub(super) struct SelectionPlugin;

impl Plugin for SelectionPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(start_selection)
            .add_observer(start_selection_at_vi_cursor)
            .add_observer(update_selection)
            .add_observer(change_selection_kind)
            .add_observer(clear_selection);
    }
}

fn start_selection(e: On<RequestTermSelectionStart>, mut terms: Query<&mut OrzmaTermHandle>) {
    if let Ok(mut tty) = terms.get_mut(e.terminal)
        && let Err(err) = tty.start_selection(e.cell, e.side, e.kind)
    {
        error!(%err);
    }
}

fn start_selection_at_vi_cursor(
    e: On<RequestTermSelectionStartAtViCursor>,
    mut terms: Query<&mut OrzmaTermHandle>,
) {
    if let Ok(mut tty) = terms.get_mut(e.terminal)
        && let Err(err) = tty.start_selection_at_vi_cursor(e.kind)
    {
        error!(%err);
    }
}

fn update_selection(e: On<RequestTermSelectionUpdate>, mut terms: Query<&mut OrzmaTermHandle>) {
    if let Ok(mut tty) = terms.get_mut(e.terminal)
        && let Err(err) = tty.update_selection(e.cell, e.side)
    {
        error!(%err);
    }
}

fn change_selection_kind(
    e: On<RequestTermSelectionKindChange>,
    mut terms: Query<&mut OrzmaTermHandle>,
) {
    if let Ok(mut tty) = terms.get_mut(e.terminal)
        && let Err(err) = tty.change_selection_kind(e.kind)
    {
        error!(%err);
    }
}

fn clear_selection(e: On<RequestTermSelectionClear>, mut terms: Query<&mut OrzmaTermHandle>) {
    if let Ok(mut tty) = terms.get_mut(e.terminal)
        && let Err(err) = tty.clear_selection()
    {
        error!(%err);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OrzmaTermHandle;
    use orzma_vt::prelude::{SelectionRange, VtBackend, VtSelection};

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

    fn start_simple(app: &mut App, terminal: Entity, x: u16, y: i16) {
        app.world_mut().trigger(RequestTermSelectionStart {
            terminal,
            cell: cell(x, y),
            side: CellSide::Left,
            kind: SelectionKind::Simple,
        });
    }

    fn update_to(app: &mut App, terminal: Entity, x: u16, y: i16, side: CellSide) {
        app.world_mut().trigger(RequestTermSelectionUpdate {
            terminal,
            cell: cell(x, y),
            side,
        });
    }

    fn selection_range(app: &mut App, terminal: Entity) -> Option<SelectionRange> {
        app.world_mut()
            .get_mut::<OrzmaTermHandle>(terminal)
            .expect("terminal entity must keep its handle")
            .vt_mut()
            .selection_range()
    }

    fn selection_kind(app: &mut App, terminal: Entity) -> Option<SelectionKind> {
        app.world_mut()
            .get_mut::<OrzmaTermHandle>(terminal)
            .expect("terminal entity must keep its handle")
            .vt_mut()
            .selection_kind()
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
    /// Case: the user presses the mouse on a cell to anchor a
    /// selection, drags across the neighboring cells to extend it, and
    /// copies the highlighted span.
    #[test]
    fn a_press_and_drag_select_the_dragged_span() {
        let (mut app, terminal) = app_with_terminal(b"abcdefghij");
        start_simple(&mut app, terminal, 0, 0);
        update_to(&mut app, terminal, 4, 0, CellSide::Right);
        assert_eq!(selected_text(&mut app, terminal).as_deref(), Some("abcde"));
    }

    /// Asserts that a clear request drops the active selection.
    ///
    /// Case: the user clicks elsewhere to dismiss an existing
    /// selection.
    #[test]
    fn clear_drops_the_active_selection() {
        let (mut app, terminal) = app_with_terminal(b"abcdefghij");
        start_simple(&mut app, terminal, 0, 0);
        update_to(&mut app, terminal, 4, 0, CellSide::Right);
        assert!(
            selection_range(&mut app, terminal).is_some(),
            "precondition: the drag must have selected something"
        );
        app.world_mut()
            .trigger(RequestTermSelectionClear { terminal });
        assert_eq!(selection_range(&mut app, terminal), None);
    }

    /// Asserts that a vi-cursor start request starts a selection in
    /// the targeted entity's VT.
    ///
    /// Case: the user presses `v` in vi mode.
    #[test]
    fn a_vi_cursor_start_request_starts_a_selection() {
        let (mut app, terminal) = app_with_terminal(b"abcdefghij");
        app.world_mut()
            .trigger(RequestTermSelectionStartAtViCursor {
                terminal,
                kind: SelectionKind::Simple,
            });
        assert_eq!(
            selection_kind(&mut app, terminal),
            Some(SelectionKind::Simple)
        );
    }

    /// Asserts that a kind-change request switches the active
    /// selection's granularity while routing through `change_selection_kind`
    /// rather than re-anchoring at the vi cursor.
    ///
    /// Case: the user presses `V` while a character-wise selection
    /// anchored on a lower row is active.
    #[test]
    fn a_kind_change_request_switches_the_granularity() {
        let (mut app, terminal) = app_with_terminal(b"abcdefghij\r\nklmnopqrst");
        start_simple(&mut app, terminal, 0, 1);
        app.world_mut().trigger(RequestTermSelectionKindChange {
            terminal,
            kind: SelectionKind::Lines,
        });
        assert_eq!(
            selection_kind(&mut app, terminal),
            Some(SelectionKind::Lines)
        );
        assert_eq!(
            selected_text(&mut app, terminal).as_deref(),
            Some("abcdefghij\nklmnopqrst\n")
        );
    }
}
