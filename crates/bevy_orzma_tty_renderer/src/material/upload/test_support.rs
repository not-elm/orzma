//! Cell and cache fixtures shared by the upload's tests.

use crate::material::{GpuCell, upload::TerminalMaterialState};
use bevy::prelude::Handle;
use orzma_vt::prelude::{Cell, HyperlinkId};

/// A narrow cell holding the first `char` of `text` as its glyph and
/// the rest as its marks, linked to `link` when given.
pub(crate) fn cell_with_link(text: &str, link: Option<u32>) -> Cell {
    let mut chars = text.chars();
    let mut cell = Cell {
        c: chars.next().expect("a fixture names a glyph"),
        hyperlink_id: link.map(|id| HyperlinkId::new(id).expect("nonzero")),
        ..Cell::default()
    };
    for mark in chars {
        assert!(cell.push_mark(mark), "a fixture stays under the mark cap");
    }
    cell
}

/// Returns the observable payload of each GPU slot as
/// `(glyph_index, fg, bg, style_flags, hyperlink_id)`.
pub(crate) fn gpu_cell_fingerprint(cells: &[GpuCell]) -> Vec<(u32, u32, u32, u32, u32)> {
    cells
        .iter()
        .map(|cell| {
            (
                cell.glyph_index,
                cell.fg_packed,
                cell.bg_packed,
                cell.style_flags,
                cell.hyperlink_id,
            )
        })
        .collect()
}

/// A cache sized for `cell_count` default cells, whose buffers are
/// unused handles.
pub(crate) fn state_for(cell_count: usize) -> TerminalMaterialState {
    TerminalMaterialState {
        cpu_cells: vec![GpuCell::default(); cell_count],
        ..TerminalMaterialState::new(Handle::default(), Handle::default())
    }
}
