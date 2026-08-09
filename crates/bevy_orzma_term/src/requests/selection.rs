//! `RequestTermSelection`: the selection operation the host UI asks a
//! terminal entity to perform.
//!
//! [`SelectionKind`] and [`CellSide`] mirror the VT's selection vocabulary.
//! Their final home is the VT layer; they are defined here until that crate
//! owns them, at which point these become re-exports.

use bevy::prelude::*;
use orzma_vt::prelude::Position;

/// Fired by the host UI to change a specific terminal entity's selection.
#[derive(EntityEvent, Debug, Clone)]
pub struct RequestTermSelection {
    #[event_target]
    pub terminal: Entity,
    /// The operation to perform.
    pub op: SelectionOp,
}

/// One selection operation.
///
/// The two `Start` variants differ in where the anchor comes from: a mouse
/// drag names an explicit cell, while vi mode anchors at the vi cursor, whose
/// position only the VT knows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SelectionOp {
    /// Anchor a new selection at an explicit viewport cell (mouse press).
    StartAt {
        /// Viewport cell, `x` = column and `y` = row, both 0-based.
        cell: Position,
        /// Which half of the cell the anchor sits in.
        side: CellSide,
        /// Granularity of the new selection.
        kind: SelectionKind,
    },
    /// Anchor a new selection at the vi cursor (vi-mode `v` / `V`).
    StartAtViCursor {
        /// Granularity of the new selection.
        kind: SelectionKind,
    },
    /// Move the moving end of the active selection to a viewport cell
    /// (mouse drag). No-op when nothing is selected.
    UpdateTo {
        /// Viewport cell, `x` = column and `y` = row, both 0-based.
        cell: Position,
        /// Which half of the cell the moving end sits in.
        side: CellSide,
    },
    /// Switch granularity while keeping the anchor (vi-mode `v` while `V` is
    /// active, and the reverse).
    ChangeKind(SelectionKind),
    /// Drop any active selection.
    Clear,
}

/// Selection granularity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionKind {
    /// Cell-by-cell, wrapping at the end of each line.
    Simple,
    /// A rectangular column block.
    Block,
    /// Snapped outward to word boundaries.
    Semantic,
    /// Whole lines.
    Lines,
}

/// Which half of a cell a selection endpoint sits in.
///
/// Decides whether the cell under the cursor is included: an endpoint on the
/// far side of a cell takes that cell, an endpoint on the near side stops
/// before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellSide {
    /// Left half.
    Left,
    /// Right half.
    Right,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `(target, op)` an observer saw, in fire order.
    #[derive(Resource, Default)]
    struct Seen(Vec<(Entity, SelectionOp)>);

    /// Observer that appends what it received to [`Seen`].
    fn record(ev: On<RequestTermSelection>, mut seen: ResMut<Seen>) {
        seen.0.push((ev.event_target(), ev.op));
    }

    fn cell(x: usize, y: usize) -> Position {
        Position { x, y }
    }

    /// Asserts that a triggered `RequestTermSelection` reaches an observer
    /// with its target and full payload intact.
    ///
    /// Case: a mouse press that anchors a new selection. The cell, the side,
    /// and the granularity all have to survive together — the apply observer
    /// cannot reconstruct any of them from the others.
    #[test]
    fn trigger_delivers_the_requested_operation() {
        let mut app = App::new();
        app.init_resource::<Seen>().add_observer(record);
        let terminal = app.world_mut().spawn_empty().id();
        let op = SelectionOp::StartAt {
            cell: cell(12, 3),
            side: CellSide::Right,
            kind: SelectionKind::Semantic,
        };

        app.world_mut()
            .trigger(RequestTermSelection { terminal, op });

        assert_eq!(app.world().resource::<Seen>().0, vec![(terminal, op)]);
    }

    /// Asserts that a full drag sequence arrives in order and unmerged.
    ///
    /// Case: press, drag across several cells, release. Each `UpdateTo` moves
    /// only the moving end, so the sequence is order-dependent; coalescing the
    /// intermediate updates — tempting, since only the last one decides the
    /// final range — would break a future consumer that reads them for
    /// autoscroll or hover feedback.
    #[test]
    fn a_drag_sequence_is_delivered_in_order() {
        let mut app = App::new();
        app.init_resource::<Seen>().add_observer(record);
        let terminal = app.world_mut().spawn_empty().id();
        let ops = [
            SelectionOp::StartAt {
                cell: cell(0, 0),
                side: CellSide::Left,
                kind: SelectionKind::Simple,
            },
            SelectionOp::UpdateTo {
                cell: cell(5, 0),
                side: CellSide::Right,
            },
            SelectionOp::UpdateTo {
                cell: cell(9, 2),
                side: CellSide::Left,
            },
        ];

        for op in ops {
            app.world_mut()
                .trigger(RequestTermSelection { terminal, op });
        }

        let seen: Vec<SelectionOp> = app
            .world()
            .resource::<Seen>()
            .0
            .iter()
            .map(|(_, op)| *op)
            .collect();
        assert_eq!(seen, ops);
    }

    /// Asserts that the two anchor sources stay distinct.
    ///
    /// Case: the vi-mode `v` press. `StartAtViCursor` deliberately carries no
    /// cell, because the vi cursor's position lives in the VT and the host
    /// does not track it. Folding it into `StartAt` with a placeholder cell
    /// would anchor every vi selection at that placeholder.
    #[test]
    fn vi_cursor_anchor_is_not_an_explicit_cell_anchor() {
        let from_vi = SelectionOp::StartAtViCursor {
            kind: SelectionKind::Lines,
        };
        let from_mouse = SelectionOp::StartAt {
            cell: cell(0, 0),
            side: CellSide::Left,
            kind: SelectionKind::Lines,
        };
        assert_ne!(from_vi, from_mouse);
    }

    /// Asserts that the payload fields are compared, not just the variant.
    ///
    /// Case: the guard for every `assert_eq!` above. A hand-written or derived
    /// `PartialEq` that ignored a field would let those tests pass while the
    /// side, the granularity, or the row silently changed in transit — and
    /// `side` in particular flips whether the cell under the cursor is part of
    /// the selection.
    #[test]
    fn operations_differing_only_in_payload_are_not_equal() {
        let base = SelectionOp::StartAt {
            cell: cell(4, 4),
            side: CellSide::Left,
            kind: SelectionKind::Simple,
        };
        assert_ne!(
            base,
            SelectionOp::StartAt {
                cell: cell(4, 4),
                side: CellSide::Right,
                kind: SelectionKind::Simple,
            }
        );
        assert_ne!(
            base,
            SelectionOp::StartAt {
                cell: cell(4, 4),
                side: CellSide::Left,
                kind: SelectionKind::Block,
            }
        );
        assert_ne!(
            base,
            SelectionOp::StartAt {
                cell: cell(4, 5),
                side: CellSide::Left,
                kind: SelectionKind::Simple,
            }
        );
    }
}
