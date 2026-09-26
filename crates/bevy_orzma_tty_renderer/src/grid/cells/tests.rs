//! Tests for the terminal cells, one file per operation under test.

use super::*;
use crate::grid::test_support::run_with_link;

mod apply;
mod hyperlink_at;
mod refill_row;

fn id(value: u32) -> HyperlinkId {
    HyperlinkId::new(value).expect("nonzero")
}

fn linked_cell(c: char, link: Option<u32>) -> Cell {
    Cell {
        c,
        hyperlink_id: link.map(id),
        ..Cell::default()
    }
}

/// The glyph and marks `cell` paints, as one string.
fn text_of(cell: &Cell) -> String {
    cell.chars().collect()
}

/// A retained table holding the single entry a linked-cell fixture
/// needs, since a cell stores only the id.
fn link_table(number: u32, uri: &str) -> HashMap<HyperlinkId, HyperlinkUri> {
    HashMap::from([(id(number), HyperlinkUri::new(uri))])
}

fn run_with_widths(text: &str, widths: &[u8]) -> Run {
    let mut run = run_with_link(text, None);
    run.cols = if widths.is_empty() {
        text.chars().count() as u16
    } else {
        widths.iter().map(|w| u16::from(*w)).sum()
    };
    run.widths = widths.to_vec();
    run
}
