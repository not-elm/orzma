//! One terminal's painted cells, materialized from the attribute runs of
//! the frames applied to it.

use crate::error::RendererResult;
use bevy::prelude::Component;
use orzma_vt::prelude::{Cell, CellWidth, Frame, HyperlinkId, HyperlinkUri, Palette, Run};
use std::collections::HashMap;

/// The painted contents of one terminal, materialized into cells from
/// the frames applied to it.
#[derive(Component, Default)]
pub struct TerminalCells {
    /// Cells indexed `[row][column]`.
    ///
    /// A [`CellWidth::Wide`] cell is followed by its
    /// [`Cell::continuation`], and a column no run covered holds
    /// [`Cell::default`]. A cell carries a hyperlink id only when
    /// [`Self::hyperlinks`] holds it.
    pub cells: Vec<Vec<Cell>>,
    /// OSC 8 hyperlinks indexed by id. Every applied frame merges its
    /// `hyperlinks` into this table, and a known id is never
    /// overwritten.
    pub hyperlinks: HashMap<HyperlinkId, HyperlinkUri>,
    /// The live palette set by the last frame that carried one;
    /// symbolic cell colors resolve against it.
    pub palette: Palette,
}

impl TerminalCells {
    /// Resolves `(row, col)` to the hyperlink at that visible cell, if
    /// any. `col` is a column coordinate: both columns of a wide cell
    /// resolve to the same hyperlink. Returns `None` for out-of-bounds
    /// or unlinked cells.
    pub fn hyperlink_at(&self, row: u16, col: u16) -> Option<(HyperlinkId, &HyperlinkUri)> {
        let id = self
            .cells
            .get(usize::from(row))?
            .get(usize::from(col))?
            .hyperlink_id?;
        Some((id, self.hyperlinks.get(&id)?))
    }

    /// Whether applying `frame` would change these cells.
    ///
    /// An absent row and a `None` section mean "unchanged", so this
    /// counts only a row inside the frame's own size, a size the rows do
    /// not hold yet, a listed palette that differs, or a hyperlink id
    /// the table lacks.
    ///
    /// # Invariants
    ///
    /// Returns `true` exactly when [`Self::apply`] mutates something or
    /// returns `Err`.
    pub fn differs_from(&self, frame: &Frame) -> bool {
        let Frame {
            size,
            rows,
            cursor: _,
            display_offset: _,
            vi_cursor: _,
            selection: _,
            placements: _,
            palette,
            hyperlinks,
        } = frame;
        self.size_differs(size.cols, size.rows)
            || rows.iter().any(|row| row.line.0 < size.rows)
            || palette
                .as_ref()
                .is_some_and(|palette| *palette != self.palette)
            || hyperlinks
                .iter()
                .any(|link| !self.hyperlinks.contains_key(&link.id))
    }

    /// Applies `frame`'s content sections to these cells.
    ///
    /// Every row the frame carries inside its size replaces the cells at
    /// that line, resolving its hyperlink ids against the table, into
    /// which this frame's own definitions are merged first; the `None`
    /// sections are left alone.
    ///
    /// # Errors
    ///
    /// A frame carrying a run that fails [`Run::check`] is rejected with
    /// that error wrapped in [`RendererError::Vt`], and the cells are left
    /// untouched.
    ///
    /// # Invariants
    ///
    /// After `apply` returns `Ok` there are exactly `frame.size.rows`
    /// rows, each holding exactly `frame.size.cols` cells. It mutates the
    /// cells exactly when [`Self::differs_from`] reports `true` and it
    /// returns `Ok`.
    ///
    /// [`RendererError::Vt`]: crate::prelude::RendererError::Vt
    pub fn apply(&mut self, frame: &Frame) -> RendererResult {
        let Frame {
            size,
            rows,
            cursor: _,
            display_offset: _,
            vi_cursor: _,
            selection: _,
            placements: _,
            palette,
            hyperlinks,
        } = frame;
        rows.iter()
            .flat_map(|row| row.contents.iter())
            .try_for_each(Run::check)?;
        let cols = usize::from(size.cols);
        self.cells
            .resize_with(usize::from(size.rows), || vec![Cell::default(); cols]);
        for link in hyperlinks {
            self.hyperlinks
                .entry(link.id)
                .or_insert_with(|| link.uri.clone());
        }
        for row in rows {
            let Some(slot) = self.cells.get_mut(usize::from(row.line.0)) else {
                continue;
            };
            refill_row(slot, &row.contents, size.cols, &self.hyperlinks);
        }
        for row in &mut self.cells {
            if row.len() != cols {
                row.clear();
                row.resize_with(cols, Cell::default);
            }
        }
        if let Some(palette) = palette {
            self.palette.clone_from(palette);
        }
        Ok(())
    }

    fn size_differs(&self, cols: u16, rows: u16) -> bool {
        self.cells.len() != usize::from(rows)
            || self.cells.iter().any(|row| row.len() != usize::from(cols))
    }
}

/// Refills `out` with exactly `cols` cells materialized from one row's
/// attribute runs, replacing whatever it held, and resolves each run's
/// hyperlink id against the retained table.
///
/// Column advance follows each run's widths. A `char` at width two takes
/// a [`CellWidth::Wide`] cell plus the [`Cell::continuation`] that
/// follows it, or one narrow cell on the last column. A `char` at width
/// zero joins the marks of the cell before it instead of taking a column,
/// and marks past [`MAX_COMBINING`] are dropped. Runs that do not fill
/// the row leave [`Cell::default`] behind, and content past the last
/// column is truncated. Every run must pass [`Run::check`].
///
/// [`MAX_COMBINING`]: orzma_vt::prelude::MAX_COMBINING
fn refill_row(
    out: &mut Vec<Cell>,
    runs: &[Run],
    cols: u16,
    hyperlinks: &HashMap<HyperlinkId, HyperlinkUri>,
) {
    out.resize_with(usize::from(cols), Cell::default);
    let mut column = 0usize;
    let mut base_column: Option<usize> = None;
    'runs: for run in runs {
        let hyperlink_id = run.hyperlink_id.filter(|id| hyperlinks.contains_key(id));
        let mut widths = run.widths.iter().copied();
        for c in run.text.chars() {
            let cell_width = widths.next().unwrap_or(1);
            if cell_width == 0 {
                if let Some(base) = base_column.and_then(|index| out.get_mut(index)) {
                    base.push_mark(c);
                }
                continue;
            }
            let Some(slot) = out.get_mut(column) else {
                break 'runs;
            };
            *slot = Cell {
                c,
                width: CellWidth::Narrow,
                extra: None,
                fg: run.fg,
                bg: run.bg,
                style: run.style,
                hyperlink_id,
            };
            base_column = Some(column);
            column += 1;
            if cell_width == 2
                && let Some([body, spacer]) = out.get_mut(column - 1..=column)
            {
                body.width = CellWidth::Wide;
                *spacer = body.continuation();
                column += 1;
            }
        }
    }
    if let Some(tail) = out.get_mut(column..) {
        tail.fill_with(Cell::default);
    }
}

#[cfg(test)]
mod tests;
