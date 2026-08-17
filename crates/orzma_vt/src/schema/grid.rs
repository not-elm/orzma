//! Grid vocabulary: [`DisplayOffset`], [`GridSize`], the active-grid
//! coordinate types [`GridLine`], [`GridColumn`], and [`GridPoint`],
//! and their viewport projection [`ViewportLine`].

pub mod cell;

/// Number of scrollback rows the viewport sits above the live tail.
///
/// `0` means the viewport is pinned to the live tail; a positive value
/// counts the scrollback rows showing above it. The unit is grid rows.
///
/// # Invariants
///
/// The producing backend keeps the value within the scrollback
/// capacity and at `0` while the alternate screen is active; this type
/// does not enforce either bound itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayOffset(pub u32);

/// Grid dimensions in cells.
///
/// The row count is the source of truth for "one screenful" (scroll
/// paging) and for verifying an applied resize.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridSize {
    /// Visible column count.
    pub cols: u16,
    /// Visible row count.
    pub rows: u16,
}

/// A line in active-grid coordinates: `0` is the top of the active
/// screen area, negative values reach into scrollback history.
///
/// It is invariant under user scrolling — moving the viewport only changes
/// [`DisplayOffset`], and the relation to a viewport row is `viewport_row = line + display_offset`.
///
/// It does move when content scrolls: a row pushed into history shifts every line of existing
/// text by `-1`. It is therefore a frame-local coordinate, not an
/// identifier that stays stable across scrollback eviction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GridLine(pub i32);

impl GridLine {
    #[inline]
    pub fn to_viewport(&self, offset: DisplayOffset, rows: u16) -> Option<ViewportLine> {
        let vl = self.0 + offset.0 as i32;
        if !(0..(rows as i32)).contains(&vl) {
            return None;
        }
        Some(ViewportLine(u16::try_from(vl).ok()?))
    }
}

#[cfg(feature = "alacritty")]
impl From<alacritty_terminal::index::Line> for GridLine {
    #[inline]
    fn from(value: alacritty_terminal::index::Line) -> Self {
        GridLine(value.0)
    }
}

/// A line in viewport coordinates: `0` is the topmost visible row.
///
/// It is the viewport projection of a [`GridLine`], related by
/// `viewport_line = grid_line + display_offset`. Negative values sit
/// above the viewport, values at or past the viewport row count sit
/// below it. Clamping off-viewport values to the `-1` / row-count
/// sentinels is the responsibility of the conversion that produces
/// the value, not of this type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ViewportLine(pub u16);

/// A 0-based grid column.
///
/// Columns are shared between the grid and viewport spaces: with no
/// horizontal scrolling, only the line axis differs between the two.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GridColumn(pub u16);

/// A cell in active-grid coordinates.
///
/// Pairs a [`GridLine`] with a [`GridColumn`]. The position does not
/// depend on where the user has scrolled the viewport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GridPoint {
    /// Line in active-grid coordinates.
    pub line: GridLine,
    /// 0-based grid column.
    pub column: GridColumn,
}

#[cfg(feature = "alacritty")]
impl From<alacritty_terminal::index::Point> for GridPoint {
    fn from(value: alacritty_terminal::index::Point) -> Self {
        Self {
            line: value.line.into(),
            column: GridColumn(value.column.0 as u16),
        }
    }
}

#[cfg(feature = "alacritty")]
impl From<GridPoint> for alacritty_terminal::index::Point {
    /// Converts literally, without clamping: a selection drag that
    /// left the viewport names a real scrollback line, and
    /// `Selection::to_range` clamps to the grid on its own.
    fn from(value: GridPoint) -> Self {
        Self {
            line: alacritty_terminal::index::Line(value.line.0),
            column: alacritty_terminal::index::Column(usize::from(value.column.0)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROWS: u16 = 24;

    /// Asserts that with a zero display offset the projection maps a
    /// grid line to the same viewport line.
    ///
    /// Case: the terminal is pinned to the live tail, where the row a
    /// cell occupies on screen is its grid line itself.
    #[test]
    fn at_the_live_tail_the_projection_is_identity() {
        let offset = DisplayOffset(0);
        assert_eq!(GridLine(0).to_viewport(offset, ROWS), Some(ViewportLine(0)));
        assert_eq!(
            GridLine(23).to_viewport(offset, ROWS),
            Some(ViewportLine(23))
        );
    }

    /// Asserts that a scrolled-back viewport shifts a history line to
    /// its on-screen row by the display offset.
    ///
    /// Case: the user scrolls back five rows, and a line that sits in
    /// scrollback history becomes visible partway down the screen.
    #[test]
    fn scrolling_back_shifts_history_lines_into_the_viewport() {
        assert_eq!(
            GridLine(-3).to_viewport(DisplayOffset(5), ROWS),
            Some(ViewportLine(2))
        );
    }

    /// Asserts that a line above the visible area projects to `None`,
    /// while the topmost visible row still projects.
    ///
    /// Case: the vi cursor rests on a history row and the user scrolls
    /// the viewport back toward the live tail, leaving that row above
    /// the window, so the renderer has no caret cell to paint.
    #[test]
    fn a_line_above_the_viewport_projects_to_none() {
        let offset = DisplayOffset(3);
        assert_eq!(GridLine(-10).to_viewport(offset, ROWS), None);
        assert_eq!(GridLine(-4).to_viewport(offset, ROWS), None);
        assert_eq!(
            GridLine(-3).to_viewport(offset, ROWS),
            Some(ViewportLine(0))
        );
    }

    /// Asserts that a line below the visible area projects to `None`,
    /// while the bottommost visible row still projects.
    ///
    /// Case: the user scrolls back while the shell keeps its caret on
    /// the last screen line, which falls below the window, so the
    /// renderer has no caret cell to paint.
    #[test]
    fn a_line_below_the_viewport_projects_to_none() {
        let offset = DisplayOffset(5);
        assert_eq!(GridLine(23).to_viewport(offset, ROWS), None);
        assert_eq!(GridLine(19).to_viewport(offset, ROWS), None);
        assert_eq!(
            GridLine(18).to_viewport(offset, ROWS),
            Some(ViewportLine(23))
        );
    }
}
