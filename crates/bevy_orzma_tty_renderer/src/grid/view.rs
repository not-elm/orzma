//! One terminal's viewport: its size, cursors, selection, scroll position
//! and webview placements.

use crate::grid::TerminalCells;
use bevy::prelude::{Component, UVec2};
use orzma_vt::prelude::{
    AnchoredPlacement, Cursor, CursorShape, DisplayOffset, Frame, GridPoint, SelectionRange,
    ViCursor,
};

/// The viewport geometry of one terminal, and where the user is looking
/// and pointing within it.
#[derive(Component, Default)]
#[require(TerminalCells)]
pub struct TerminalView {
    /// Visible column count.
    pub cols: u16,
    /// Visible row count.
    pub rows: u16,
    /// Current cursor state, absent until the first frame arrives.
    pub cursor: Option<Cursor>,
    /// Vi-mode cursor from the last applied frame; `None` when that frame
    /// carries none.
    pub vi_cursor: Option<ViCursor>,
    /// Active selection range from the last applied frame, independent of
    /// `vi_cursor`.
    pub selection: Option<SelectionRange>,
    /// Lines scrolled back from the live tail; 0 = at live tail.
    pub display_offset: u32,
    /// App-level cursor visibility override. When `true`, the caret
    /// is not painted. It is independent of `Cursor.visible`, so it
    /// hides the cursor without clobbering terminal-controlled state.
    pub suppress_cursor: bool,
    /// Webview placements in active-grid coordinates, mirrored from the
    /// last applied frame. Every frame that carries a list replaces
    /// this one wholesale, and a placement absent from that list has no
    /// live anchor this frame.
    pub placements: Vec<AnchoredPlacement>,
}

impl TerminalView {
    /// Projects the cursor into viewport cells as `(column, row)`, or
    /// `None` when no frame has arrived yet or the cursor's line is
    /// scrolled out of the visible rows.
    pub fn cursor_viewport_cell(&self) -> Option<(u16, u16)> {
        self.project(self.cursor.as_ref()?.point)
    }

    /// Projects the cursor into viewport cells as `(column, row)`, but
    /// never yields `None`: a cursor whose line is scrolled out of the
    /// visible rows keeps its column on row `0`, and a missing cursor maps
    /// to the origin.
    pub fn cursor_viewport_cell_or_top(&self) -> (u16, u16) {
        let Some(cursor) = self.cursor.as_ref() else {
            return (0, 0);
        };
        self.project(cursor.point)
            .unwrap_or((cursor.point.column.0, 0))
    }

    /// The cursor this view paints and the viewport cell it occupies,
    /// or `None` when no cursor projects into the viewport.
    ///
    /// The vi cursor takes precedence over the live cursor and is
    /// reported as a visible steady block.
    pub fn caret(&self) -> Option<(UVec2, Cursor)> {
        let cursor = match self.vi_cursor {
            Some(vc) => Cursor {
                point: vc.point,
                shape: CursorShape::Block,
                blinking: false,
                visible: true,
            },
            None => *self.cursor.as_ref()?,
        };
        let (column, row) = self.project(cursor.point)?;
        Some((UVec2::new(u32::from(column), u32::from(row)), cursor))
    }

    /// Whether applying `frame` would change this view.
    ///
    /// A `None` section means "unchanged", so a `None` placements list
    /// does not count.
    ///
    /// # Invariants
    ///
    /// Returns `true` exactly when [`Self::apply`] mutates something.
    pub fn differs_from(&self, frame: &Frame) -> bool {
        let Frame {
            size,
            rows: _,
            cursor,
            display_offset,
            vi_cursor,
            selection,
            placements,
            palette: _,
            hyperlinks: _,
        } = frame;
        self.cols != size.cols
            || self.rows != size.rows
            || self.cursor != Some(*cursor)
            || self.display_offset != display_offset.0
            || self.vi_cursor != *vi_cursor
            || self.selection != *selection
            || placements
                .as_ref()
                .is_some_and(|placements| *placements != self.placements)
    }

    /// Applies `frame`'s viewport sections to this view.
    ///
    /// The `None` sections are left alone, and `suppress_cursor` is
    /// never written.
    ///
    /// # Invariants
    ///
    /// It mutates the view exactly when [`Self::differs_from`] reports
    /// `true`.
    pub fn apply(&mut self, frame: &Frame) {
        let Frame {
            size,
            rows: _,
            cursor,
            display_offset,
            vi_cursor,
            selection,
            placements,
            palette: _,
            hyperlinks: _,
        } = frame;
        self.cols = size.cols;
        self.rows = size.rows;
        self.cursor = Some(*cursor);
        self.display_offset = display_offset.0;
        self.vi_cursor = *vi_cursor;
        self.selection = *selection;
        if let Some(placements) = placements {
            self.placements.clone_from(placements);
        }
    }

    /// Projects a grid point into viewport cells as `(column, row)`, or
    /// `None` when its line is scrolled out of the visible rows.
    fn project(&self, point: GridPoint) -> Option<(u16, u16)> {
        let row = point
            .line
            .to_viewport(DisplayOffset(self.display_offset), self.rows)?;
        Some((point.column.0, row.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::test_support::{dirty_row, quiet_frame};
    use orzma_vt::prelude::{
        GridColumn, GridLine, GridSize, InstanceId, PlacementSize, SelectionGeometry,
    };

    fn visible_block_cursor() -> Cursor {
        Cursor {
            point: GridPoint {
                line: GridLine(5),
                column: GridColumn(3),
            },
            shape: CursorShape::Block,
            blinking: false,
            visible: true,
        }
    }

    /// Asserts that a live cursor is reported with the viewport cell
    /// it projects to.
    ///
    /// Case: the shell sits at an ordinary prompt with the caret
    /// shown.
    #[test]
    fn caret_reports_the_live_cursor_and_its_viewport_cell() {
        let view = TerminalView {
            rows: 24,
            cursor: Some(visible_block_cursor()),
            ..Default::default()
        };
        let (pos, cursor) = view.caret().expect("the cursor projects");
        assert_eq!(pos, UVec2::new(3, 5));
        assert!(cursor.visible);
    }

    /// Asserts that `cursor_viewport_cell` follows the display offset and
    /// yields `None` once the cursor's line scrolls out of view.
    ///
    /// Case: the user scrolls back through history while the shell's
    /// cursor sits on the bottom row of a 24-row screen.
    #[test]
    fn cursor_viewport_cell_follows_the_display_offset() {
        let mut view = TerminalView {
            cols: 80,
            rows: 24,
            ..TerminalView::default()
        };
        view.cursor = Some(Cursor {
            point: GridPoint {
                line: GridLine(23),
                column: GridColumn(5),
            },
            ..Cursor::default()
        });
        assert_eq!(view.cursor_viewport_cell(), Some((5, 23)));

        view.display_offset = 3;
        assert_eq!(view.cursor_viewport_cell(), None);

        view.cursor = Some(Cursor {
            point: GridPoint {
                line: GridLine(10),
                column: GridColumn(0),
            },
            ..Cursor::default()
        });
        assert_eq!(view.cursor_viewport_cell(), Some((0, 13)));
    }

    /// Asserts that the `_or_top` fallback keeps the cursor's column on row
    /// `0` when the line is scrolled out of view, and yields the origin when
    /// no cursor exists.
    ///
    /// Case: the user scrolls back through history during an IME
    /// composition, pushing the prompt's cursor row out of the viewport.
    #[test]
    fn cursor_viewport_cell_or_top_keeps_the_column_when_off_viewport() {
        let mut view = TerminalView {
            cols: 80,
            rows: 24,
            ..TerminalView::default()
        };
        assert_eq!(view.cursor_viewport_cell_or_top(), (0, 0));

        view.cursor = Some(Cursor {
            point: GridPoint {
                line: GridLine(23),
                column: GridColumn(5),
            },
            ..Cursor::default()
        });
        view.display_offset = 3;
        assert_eq!(view.cursor_viewport_cell_or_top(), (5, 0));
    }

    /// Asserts that the vi cursor is reported ahead of the live one,
    /// as a visible steady block at its own projected cell.
    ///
    /// Case: the user enters vi mode and moves the vi caret away from
    /// the shell's own cursor.
    #[test]
    fn caret_prefers_the_vi_cursor() {
        let view = TerminalView {
            rows: 24,
            cursor: Some(visible_block_cursor()),
            vi_cursor: Some(ViCursor {
                point: GridPoint {
                    line: GridLine(2),
                    column: GridColumn(7),
                },
            }),
            ..Default::default()
        };
        let (pos, cursor) = view.caret().expect("the vi cursor projects");
        assert_eq!(pos, UVec2::new(7, 2));
        assert_eq!(cursor.shape, CursorShape::Block);
        assert!(cursor.visible);
        assert!(!cursor.blinking);
    }

    /// Asserts that a cursor whose line projects outside the viewport
    /// is reported as absent rather than clamped to an edge cell it
    /// does not occupy.
    ///
    /// Case: the user scrolls back through history while the shell
    /// keeps its caret on the live prompt line below the viewport.
    #[test]
    fn a_scrolled_away_cursor_projects_to_nothing() {
        let view = TerminalView {
            rows: 24,
            display_offset: 30,
            cursor: Some(visible_block_cursor()),
            ..Default::default()
        };
        assert_eq!(view.caret(), None);
    }

    /// Asserts that a moved cursor is a difference, and that applying
    /// the frame settles the view so the cursor round-trips to a
    /// matching state.
    ///
    /// Case: the user presses an arrow key and the application moves
    /// the caret without repainting a cell.
    #[test]
    fn a_moved_cursor_differs() {
        let mut view = TerminalView::settled();
        let frame = Frame {
            cursor: Cursor {
                point: GridPoint {
                    line: GridLine(1),
                    column: GridColumn(2),
                },
                ..Cursor::default()
            },
            ..quiet_frame()
        };
        assert!(view.differs_from(&frame));
        view.apply(&frame);
        assert!(!view.differs_from(&frame));
    }

    /// Asserts that a moved viewport (display offset) is a difference,
    /// and that applying the frame settles the view so the offset
    /// round-trips to a matching state.
    ///
    /// Case: a frame that only moves the display offset reaches a settled
    /// mirror.
    #[test]
    fn a_moved_viewport_differs() {
        let mut view = TerminalView::settled();
        let frame = Frame {
            display_offset: DisplayOffset(7),
            ..quiet_frame()
        };
        assert!(view.differs_from(&frame));
        view.apply(&frame);
        assert!(!view.differs_from(&frame));
    }

    /// Asserts that a placements list replaces the mirror wholesale,
    /// including down to empty, and that `None` leaves it alone.
    ///
    /// Case: a program mounts a webview, a quiet frame follows, and the
    /// program then unmounts it, so the next frame carries an empty
    /// list.
    #[test]
    fn placements_replace_wholesale_and_none_keeps_them() {
        let mut view = TerminalView::settled();
        let placed = AnchoredPlacement {
            id: InstanceId(1),
            point: GridPoint {
                line: GridLine(2),
                column: GridColumn(3),
            },
            size: PlacementSize { rows: 4, cols: 5 },
        };
        let mounted = Frame {
            placements: Some(vec![placed]),
            ..quiet_frame()
        };
        assert!(view.differs_from(&mounted));
        view.apply(&mounted);
        assert_eq!(view.placements, vec![placed]);

        let unchanged = quiet_frame();
        assert!(!view.differs_from(&unchanged));
        view.apply(&unchanged);
        assert_eq!(view.placements, vec![placed]);

        let hidden = Frame {
            placements: Some(vec![]),
            ..quiet_frame()
        };
        assert!(view.differs_from(&hidden));
        view.apply(&hidden);
        assert_eq!(view.placements, vec![]);
    }

    /// Asserts that a frame the view reports no difference for leaves
    /// every view field untouched.
    ///
    /// Case: a frame repeats the viewport state the view already holds,
    /// with the cursor away from the origin, a vi cursor active, a
    /// selection in progress, and the viewport scrolled into history.
    #[test]
    fn a_view_that_reports_no_difference_is_not_mutated_by_apply() {
        let cursor = Cursor {
            point: GridPoint {
                line: GridLine(2),
                column: GridColumn(3),
            },
            ..Cursor::default()
        };
        let vi_cursor = ViCursor {
            point: GridPoint {
                line: GridLine(4),
                column: GridColumn(5),
            },
        };
        let selection = SelectionRange {
            start: GridPoint {
                line: GridLine(0),
                column: GridColumn(0),
            },
            end: GridPoint {
                line: GridLine(1),
                column: GridColumn(2),
            },
            geometry: SelectionGeometry::Linear,
        };
        let placed = AnchoredPlacement {
            id: InstanceId(9),
            point: GridPoint {
                line: GridLine(1),
                column: GridColumn(1),
            },
            size: PlacementSize { rows: 1, cols: 1 },
        };
        let mut view = TerminalView {
            cols: 7,
            rows: 6,
            cursor: Some(cursor),
            vi_cursor: Some(vi_cursor),
            selection: Some(selection),
            display_offset: 4,
            placements: vec![placed],
            ..Default::default()
        };
        let frame = Frame {
            size: GridSize { cols: 7, rows: 6 },
            cursor,
            display_offset: DisplayOffset(4),
            vi_cursor: Some(vi_cursor),
            selection: Some(selection),
            placements: Some(vec![placed]),
            ..quiet_frame()
        };
        assert!(!view.differs_from(&frame));
        let before = (
            view.cols,
            view.rows,
            view.cursor,
            view.display_offset,
            view.vi_cursor,
            view.selection,
            view.placements.clone(),
        );
        view.apply(&frame);
        assert_eq!(
            (
                view.cols,
                view.rows,
                view.cursor,
                view.display_offset,
                view.vi_cursor,
                view.selection,
                view.placements.clone(),
            ),
            before
        );
    }

    /// Asserts that a size change alone is a difference for the view,
    /// and that applying the frame updates its column and row counts
    /// even though the frame carries no rows of its own.
    ///
    /// Case: a frame that only changes the size reaches a settled view.
    #[test]
    fn a_new_size_alone_differs() {
        let mut view = TerminalView::settled();
        let frame = Frame {
            size: GridSize { cols: 3, rows: 2 },
            ..quiet_frame()
        };
        assert!(view.differs_from(&frame));
        view.apply(&frame);
        assert_eq!((view.cols, view.rows), (3, 2));
        assert!(!view.differs_from(&frame));
    }

    /// Asserts that a shrinking size change is a difference for the
    /// view, and that applying the frame updates its column and row
    /// counts to the smaller size.
    ///
    /// Case: the user drags the window shorter and the VT's repaint at
    /// the new height arrives.
    #[test]
    fn a_shrinking_size_updates_the_view_dims() {
        let mut view = TerminalView {
            cols: 1,
            rows: 3,
            ..TerminalView::settled()
        };
        let frame = Frame {
            rows: vec![dirty_row(0, "a")],
            ..quiet_frame()
        };
        assert!(view.differs_from(&frame));
        view.apply(&frame);
        assert_eq!((view.cols, view.rows), (1, 1));
    }

    /// Asserts that a frame changing only the column count is a
    /// difference for the view, and that applying it updates the
    /// column count while leaving the row count alone.
    ///
    /// Case: the user drags the window wider without changing its
    /// height, and the VT's repaint at the new width arrives.
    #[test]
    fn a_cols_only_size_change_differs_and_updates_the_view() {
        let mut view = TerminalView::settled();
        let frame = Frame {
            size: GridSize { cols: 3, rows: 1 },
            rows: vec![dirty_row(0, "abc")],
            ..quiet_frame()
        };
        assert!(view.differs_from(&frame));
        view.apply(&frame);
        assert_eq!((view.cols, view.rows), (3, 1));
        assert!(!view.differs_from(&Frame {
            size: GridSize { cols: 3, rows: 1 },
            ..quiet_frame()
        }));
    }
}
