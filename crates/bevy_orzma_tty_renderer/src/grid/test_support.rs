//! Frame and grid fixtures shared by the grid's tests.

use crate::grid::{TerminalCells, TerminalView};
use orzma_vt::prelude::{
    Cell, Color, Cursor, DirtyRow, DisplayOffset, Frame, GridSize, HyperlinkId, Row, Run, Style,
    ViewportLine,
};

impl TerminalView {
    /// A one-by-one view that already mirrors [`quiet_frame`].
    pub(crate) fn settled() -> Self {
        Self {
            cols: 1,
            rows: 1,
            cursor: Some(Cursor::default()),
            ..Default::default()
        }
    }
}

impl TerminalCells {
    /// A one-by-one cell grid that already mirrors [`quiet_frame`].
    pub(crate) fn settled() -> Self {
        Self {
            cells: vec![vec![Cell::default()]],
            ..Default::default()
        }
    }
}

/// A frame for a one-by-one grid that changes nothing on its own: no
/// rows, `None` sections, the default cursor at offset zero.
pub(crate) fn quiet_frame() -> Frame {
    Frame {
        size: GridSize { cols: 1, rows: 1 },
        rows: vec![],
        cursor: Cursor::default(),
        display_offset: DisplayOffset(0),
        vi_cursor: None,
        selection: None,
        placements: None,
        palette: None,
        hyperlinks: vec![],
    }
}

pub(crate) fn run_with_link(text: &str, hyperlink_id: Option<HyperlinkId>) -> Run {
    Run {
        cols: 1,
        fg: Color::DefaultForeground,
        bg: Color::DefaultBackground,
        style: Style::empty(),
        text: text.to_string(),
        widths: Vec::new(),
        hyperlink_id,
    }
}

pub(crate) fn dirty_row(line: u16, text: &str) -> DirtyRow {
    DirtyRow {
        line: ViewportLine(line),
        contents: Row::from(vec![run_with_link(text, None)]),
    }
}
