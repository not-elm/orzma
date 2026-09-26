//! Encoding a pane's cells into the GPU cell table, resolving each glyph
//! through the atlas.

use crate::{
    font::{FontFace, TerminalFonts},
    glyph::{GlyphAtlas, GlyphKey},
    grid::TerminalCells,
    material::{
        GpuCell, GpuGlyph, STYLE_WIDE_RIGHT_HALF,
        upload::{TerminalMaterialState, palette::PackedPalette},
    },
};
use orzma_vt::prelude::{Cell, CellWidth, HyperlinkId, Style};

/// Writes every visible cell's glyph index, color and style into the CPU
/// cell table, resolving each glyph through the atlas as it goes.
///
/// `dims` is `(cols, rows)` in cells. A row or column of `cells` outside
/// it is skipped without resolving its glyphs. A [`CellWidth::Spacer`]
/// repeats the body cell directly to its left as the flagged right half;
/// a spacer without one leaves its slot at the default.
pub fn fill_cells(
    state: &mut TerminalMaterialState,
    atlas: &mut GlyphAtlas,
    cells: &TerminalCells,
    fonts: &TerminalFonts,
    phys_font_size: u16,
    dims: (u32, u32),
) {
    let (cols, rows) = dims;
    let packed_palette = PackedPalette::build(&cells.palette);
    for (row_idx, row) in cells.cells.iter().enumerate().take(rows as usize) {
        let mut left_half: Option<GpuCell> = None;
        for (col, cell) in row.iter().enumerate().take(cols as usize) {
            let col = col as u32;
            let target = (row_idx as u32 * cols + col) as usize;
            match cell.width {
                CellWidth::Narrow | CellWidth::Wide | CellWidth::LeadingSpacer => {
                    let gpu = GpuCell {
                        glyph_index: resolve_glyph_index(state, atlas, cell, fonts, phys_font_size),
                        fg_packed: packed_palette.cell_fg(cell.fg),
                        bg_packed: packed_palette.cell_bg(cell.bg),
                        style_flags: u32::from(
                            (cell.style | style_from_combining_marks(cell)).bits(),
                        ),
                        hyperlink_id: cell.hyperlink_id.map_or(0, HyperlinkId::get),
                    };
                    if let Some(target) = state.cpu_cells.get_mut(target) {
                        *target = gpu;
                    }
                    left_half = Some(gpu);
                }
                CellWidth::Spacer => {
                    if let Some(left) = left_half.take()
                        && let Some(target) = state.cpu_cells.get_mut(target)
                    {
                        *target = GpuCell {
                            style_flags: left.style_flags | STYLE_WIDE_RIGHT_HALF,
                            ..left
                        };
                    }
                }
            }
        }
    }
}

/// The line the shader draws for a combining mark, if any.
///
/// Maps U+0332 (combining low line), U+0333 (double low line), U+0331
/// (combining macron below) to `Style::UNDERLINE`, and U+0336 (combining
/// long stroke overlay) to `Style::STRIKE`.
fn line_style(mark: char) -> Option<Style> {
    match mark {
        '\u{0332}' | '\u{0333}' | '\u{0331}' => Some(Style::UNDERLINE),
        '\u{0336}' => Some(Style::STRIKE),
        _ => None,
    }
}

/// Promotes the characters of a cell's glyph and marks that stand for
/// lines to the `Style` underline and strike flags so the shader paints
/// them.
fn style_from_combining_marks(cell: &Cell) -> Style {
    cell.chars()
        .filter_map(line_style)
        .fold(Style::empty(), |acc, s| acc | s)
}

/// The marks composed onto a cell's glyph: its combining marks, except
/// the marks [`line_style`] maps to a line.
fn composable_marks(cell: &Cell) -> impl Iterator<Item = char> + '_ {
    cell.marks()
        .iter()
        .copied()
        .filter(|c| line_style(*c).is_none())
}

fn resolve_glyph_index(
    state: &mut TerminalMaterialState,
    atlas: &mut GlyphAtlas,
    cell: &Cell,
    fonts: &TerminalFonts,
    phys_font_size: u16,
) -> u32 {
    if matches!(cell.c, '\0' | ' ') || cell.chars().all(char::is_whitespace) {
        return u32::MAX;
    }
    let face = FontFace::from_style(cell.style);
    let key =
        GlyphKey::new(face, u32::from(cell.c), phys_font_size).with_marks(composable_marks(cell));
    if let Some(&idx) = state.glyph_index_map.get(&key) {
        return idx;
    }
    let Some(rect) = atlas.get_or_insert(key, fonts) else {
        return u32::MAX;
    };
    let idx = state.cpu_glyphs.len() as u32;
    state.cpu_glyphs.push(GpuGlyph::new(rect));
    state.glyph_index_map.insert(key, idx);
    idx
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material::upload::test_support::{cell_with_link, gpu_cell_fingerprint, state_for};

    /// Asserts the GPU slots a row of a wide char, a combining mark and a
    /// linked cell produces, pinning the payload of every slot including
    /// the wide char's right half.
    ///
    /// Case: a file listing hyperlinks a CJK filename so it opens on
    /// click, while an accented latin suffix typed right after it stays
    /// plain, unlinked text.
    #[test]
    fn filling_pins_wide_combining_and_linked_slots() {
        let wide = Cell {
            width: CellWidth::Wide,
            ..cell_with_link("あ", Some(3))
        };
        let continuation = wide.continuation();
        let combining = cell_with_link("e\u{0332}", None);
        let plain = cell_with_link("z", None);
        let cells = TerminalCells {
            cells: vec![vec![wide, continuation, combining, plain]],
            ..Default::default()
        };
        let mut state = state_for(4);
        let mut atlas = GlyphAtlas::default();
        let fonts = TerminalFonts::default();

        fill_cells(&mut state, &mut atlas, &cells, &fonts, 16, (4, 1));

        let fingerprint = gpu_cell_fingerprint(&state.cpu_cells);
        assert_eq!(
            fingerprint[0].4, 3,
            "the wide cell carries its hyperlink id"
        );
        assert_eq!(
            fingerprint[1].4, 3,
            "the wide cell's right half repeats the hyperlink id"
        );
        assert_eq!(
            fingerprint[1].0, fingerprint[0].0,
            "the right half repeats the left half's glyph"
        );
        assert_ne!(
            fingerprint[1].3 & STYLE_WIDE_RIGHT_HALF,
            0,
            "the right half is flagged"
        );
        assert_eq!(fingerprint[2].4, 0, "the combining cell is unlinked");
        assert_eq!(fingerprint[3].4, 0, "the plain cell is unlinked");
        assert_eq!(
            fingerprint,
            vec![
                (0, u32::MAX, 0, 0, 3),
                (0, u32::MAX, 0, STYLE_WIDE_RIGHT_HALF, 3),
                (1, u32::MAX, 0, 4, 0),
                (2, u32::MAX, 0, 0, 0),
            ]
        );
    }

    /// Asserts that a glyph and marks that are all whitespace, and a NUL
    /// or space glyph under any marks, resolve to no glyph.
    ///
    /// Case: a row holds an ideographic space, a space carrying a stray
    /// accent, a NUL an application left behind, and an untouched column.
    #[test]
    fn cells_that_paint_no_glyph_resolve_none() {
        let cells = TerminalCells {
            cells: vec![vec![
                cell_with_link("\u{3000}", None),
                cell_with_link(" \u{0301}", None),
                cell_with_link("\0", None),
                Cell::default(),
            ]],
            ..Default::default()
        };
        let mut state = state_for(4);
        let mut atlas = GlyphAtlas::default();
        let fonts = TerminalFonts::default();

        fill_cells(&mut state, &mut atlas, &cells, &fonts, 16, (4, 1));

        assert!(
            state
                .cpu_cells
                .iter()
                .all(|cell| cell.glyph_index == u32::MAX)
        );
        assert!(state.cpu_glyphs.is_empty());
    }

    /// Asserts that a linked cell's wire id reaches its GPU slot while
    /// an unlinked cell's slot keeps the 0 sentinel.
    ///
    /// Case: a row mixes OSC 8 linked text with plain text.
    #[test]
    fn filling_writes_the_hyperlink_id_when_present() {
        let linked = cell_with_link("x", Some(7));
        let unlinked = cell_with_link("y", None);
        let cells = TerminalCells {
            cells: vec![vec![linked, unlinked]],
            ..Default::default()
        };
        let mut state = state_for(2);
        let mut atlas = GlyphAtlas::default();
        let fonts = TerminalFonts::default();

        fill_cells(&mut state, &mut atlas, &cells, &fonts, 16, (2, 1));

        assert_eq!(state.cpu_cells[0].hyperlink_id, 7);
        assert_eq!(state.cpu_cells[1].hyperlink_id, 0);
    }

    /// Asserts that the underline-like combining marks promote to
    /// `Style::UNDERLINE` and the long stroke overlay to `Style::STRIKE`,
    /// whether they are the glyph itself or a mark on it, while other text
    /// and a continuation column promote to no flags.
    ///
    /// Case: a program decorates text with combining low lines and stroke
    /// overlays next to plain ASCII and accented text.
    #[test]
    fn combining_marks_promote_to_underline_and_strike() {
        let style_of = |text: &str| style_from_combining_marks(&cell_with_link(text, None));
        assert_eq!(style_of("a"), Style::empty());
        assert_eq!(style_of("e\u{0301}"), Style::empty());
        for mark in ['\u{0331}', '\u{0332}', '\u{0333}'] {
            assert_eq!(style_of(&format!("a{mark}")), Style::UNDERLINE);
        }
        assert_eq!(style_of("a\u{0336}"), Style::STRIKE);
        assert_eq!(
            style_of("a\u{0332}\u{0336}"),
            Style::UNDERLINE | Style::STRIKE
        );
        assert_eq!(style_of("\u{0332}"), Style::UNDERLINE);
        let spacer = Cell {
            width: CellWidth::Spacer,
            ..cell_with_link("\u{0332}", None)
        };
        assert_eq!(style_from_combining_marks(&spacer), Style::empty());
    }

    /// Asserts that the marks the shader draws as lines are left out of
    /// the composable set, and that the base glyph is never one of them.
    ///
    /// Case: a cell holds `e` with an acute accent, a combining low line,
    /// a long stroke overlay and a tilde.
    #[test]
    fn composable_marks_skip_the_base_and_the_line_marks() {
        let decorated = cell_with_link("e\u{0301}\u{0332}\u{0336}\u{0303}", None);
        assert_eq!(
            composable_marks(&decorated).collect::<Vec<_>>(),
            ['\u{0301}', '\u{0303}']
        );
        assert_eq!(composable_marks(&cell_with_link("a", None)).count(), 0);
        assert_eq!(
            composable_marks(&cell_with_link("\u{0301}", None)).count(),
            0
        );
    }
}
