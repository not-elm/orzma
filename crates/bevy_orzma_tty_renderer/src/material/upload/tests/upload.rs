//! Tests for rebuilding one pane's cell and glyph buffers.

use super::*;
use crate::material::upload::{
    palette::{PackedPalette, TRANSPARENT_BG},
    test_support::{cell_with_link, gpu_cell_fingerprint, state_for},
};
use orzma_vt::prelude::{Color as CellColor, Palette};

/// The fingerprint a default cell packs to under the default palette:
/// no glyph, the palette foreground, the transparent background, no
/// style and no link.
fn blank_fingerprint() -> (u32, u32, u32, u32, u32) {
    let fg = PackedPalette::build(&Palette::default()).cell_fg(CellColor::DefaultForeground);
    (u32::MAX, fg, TRANSPARENT_BG, 0, 0)
}

fn hyperlink_ids(cells: &[GpuCell]) -> Vec<u32> {
    cells.iter().map(|cell| cell.hyperlink_id).collect()
}

fn grid_of(rows: usize, cols: usize) -> TerminalCells {
    TerminalCells {
        cells: vec![vec![cell_with_link("x", None); cols]; rows],
        ..Default::default()
    }
}

/// Two empty buffer assets and a cache that uploads into them.
fn buffers_and_state() -> (Assets<ShaderBuffer>, TerminalMaterialState) {
    let mut buffers = Assets::<ShaderBuffer>::default();
    let cells = buffers.add(ShaderBuffer::default());
    let glyphs = buffers.add(ShaderBuffer::default());
    (buffers, TerminalMaterialState::new(cells, glyphs))
}

fn basis_at(dims: (u16, u16), atlas: &GlyphAtlas, phys_font_size: u16) -> UploadBasis {
    UploadBasis {
        dims,
        atlas_restarts: atlas.restarts,
        phys_font_size,
    }
}

/// Uploads `cells` at `dims` against a fresh atlas at 16 px.
fn uploaded(
    state: &mut TerminalMaterialState,
    buffers: &mut Assets<ShaderBuffer>,
    cells: &TerminalCells,
    dims: (u16, u16),
) {
    let mut atlas = GlyphAtlas::default();
    let fonts = TerminalFonts::default();
    let basis = basis_at(dims, &atlas, 16);
    state
        .upload(&mut atlas, buffers, cells, &fonts, basis)
        .expect("both buffers exist");
}

/// Asserts that an upload ignores the rows and columns outside its
/// dimensions without resolving their glyphs, and leaves the slots
/// the retained cells do not cover at their default, instead of
/// panicking on the mismatch.
///
/// Case: a malformed frame that also resizes the pane is rejected by
/// the cells while the view takes the new size, so the next rebuilds
/// see a grid of another shape than the view reports.
#[test]
fn an_upload_clips_a_grid_that_disagrees_with_its_dims() {
    let (mut buffers, mut state) = buffers_and_state();
    let linked = |id| cell_with_link("x", Some(id));
    let larger = TerminalCells {
        cells: vec![
            vec![linked(1), linked(2), linked(3)],
            vec![Cell::default(), linked(4)],
            vec![linked(5)],
            vec![cell_with_link("y", Some(6))],
        ],
        ..Default::default()
    };
    let smaller = TerminalCells {
        cells: vec![vec![linked(7)]],
        ..Default::default()
    };
    let untouched = gpu_cell_fingerprint(&[GpuCell::default()])[0];

    uploaded(&mut state, &mut buffers, &larger, (2, 3));
    assert_eq!(hyperlink_ids(&state.cpu_cells), [1, 2, 0, 4, 5, 0]);
    let fingerprint = gpu_cell_fingerprint(&state.cpu_cells);
    assert_eq!(fingerprint[2], blank_fingerprint());
    assert_eq!(fingerprint[5], untouched);
    assert_eq!(
        state.cpu_glyphs.len(),
        1,
        "the row outside the dimensions resolves no glyph"
    );

    uploaded(&mut state, &mut buffers, &smaller, (2, 3));
    assert_eq!(hyperlink_ids(&state.cpu_cells), [7, 0, 0, 0, 0, 0]);
    let fingerprint = gpu_cell_fingerprint(&state.cpu_cells);
    assert!(fingerprint[1..].iter().all(|slot| *slot == untouched));
}

/// Asserts that every upload leaves the CPU cell table holding that
/// upload's cells alone, and hands the buffer asset their encoding.
///
/// Case: a pane redraws at an unchanged size, and the second frame
/// blanks a cell that the first one painted.
#[test]
fn an_upload_keeps_its_cpu_cell_table_across_uploads() {
    let (mut buffers, mut state) = buffers_and_state();
    let painted = grid_of(2, 2);
    let mut blanked = grid_of(2, 2);
    blanked.cells[1][1] = Cell::default();

    for cells in [&painted, &blanked] {
        uploaded(&mut state, &mut buffers, cells, (2, 2));
        assert_eq!(state.cpu_cells.len(), 4);
        let mut expected = ShaderBuffer::default();
        expected.set_data(&state.cpu_cells);
        let encoded = buffers
            .get(&state.cells_buffer)
            .and_then(|buffer| buffer.data.as_ref());
        assert_eq!(encoded, expected.data.as_ref());
    }
    assert_eq!(
        gpu_cell_fingerprint(&state.cpu_cells)[3],
        blank_fingerprint()
    );
}

/// Asserts that a pane needs an upload before its first build, when its
/// cells changed, and when any part of its basis moved, and not
/// otherwise.
///
/// Case: a pane is drawn for the first time and then sits idle, until it
/// receives output, is resized, sees the glyph atlas restart, and has
/// its font zoomed.
#[test]
fn a_pane_needs_an_upload_exactly_when_its_cells_or_basis_changed() {
    let built = UploadBasis {
        dims: (80, 24),
        atlas_restarts: 0,
        phys_font_size: 24,
    };
    let mut state = state_for(0);
    assert!(state.needs_upload(built, false), "never built");
    state.uploaded = Some(built);
    assert!(!state.needs_upload(built, false));
    assert!(state.needs_upload(built, true));
    for moved in [
        UploadBasis {
            dims: (81, 24),
            ..built
        },
        UploadBasis {
            atlas_restarts: 1,
            ..built
        },
        UploadBasis {
            phys_font_size: 12,
            ..built
        },
    ] {
        assert!(state.needs_upload(moved, false), "{moved:?}");
    }
}

/// Asserts that an upload keeps the glyph table when only the size
/// changed, and drops it when the atlas restarted or the font size
/// changed since the recorded build.
///
/// Case: a pane is resized, then the shared atlas fills up and
/// restarts, and then the user zooms the font.
#[test]
fn an_upload_keeps_the_glyph_table_only_while_atlas_and_font_size_hold() {
    let fonts = TerminalFonts::default();
    let mut atlas = GlyphAtlas::default();
    let (mut buffers, mut state) = buffers_and_state();
    let steps = [
        ((1, 1), "x", 0, 16, 1, "the first build"),
        ((2, 1), "y", 0, 16, 2, "a resize keeps the table"),
        ((2, 1), "y", 1, 16, 1, "an atlas restart drops the table"),
        ((2, 1), "x", 1, 24, 1, "a font size change drops the table"),
    ];
    for (dims, text, restarts, phys_font_size, glyphs, why) in steps {
        atlas.restarts = restarts;
        let basis = basis_at(dims, &atlas, phys_font_size);
        state
            .upload(&mut atlas, &mut buffers, &row_of(text), &fonts, basis)
            .expect("both buffers exist");
        assert_eq!(state.glyph_index_map.len(), glyphs, "{why}");
    }
}

/// Asserts that an upload with no recorded build drops the glyph table.
///
/// Case: a pane's previous upload failed, and its next frame shows
/// different glyphs.
#[test]
fn an_upload_without_a_recorded_basis_drops_the_glyph_table() {
    let fonts = TerminalFonts::default();
    let mut atlas = GlyphAtlas::default();
    let (mut buffers, mut state) = buffers_and_state();
    let basis = basis_at((1, 1), &atlas, 16);
    state
        .upload(&mut atlas, &mut buffers, &row_of("x"), &fonts, basis)
        .expect("both buffers exist");
    state.uploaded = None;
    state
        .upload(&mut atlas, &mut buffers, &row_of("y"), &fonts, basis)
        .expect("both buffers exist");
    assert_eq!(state.glyph_index_map.len(), 1);
}

/// Asserts that an upload during which the atlas restarts records no
/// basis, rather than recording glyph indices into rects the restart
/// evicted.
///
/// Case: a row's first glyph is already cached in a nearly full atlas,
/// and its second glyph overflows the atlas mid-build.
#[test]
fn an_upload_that_restarts_the_atlas_records_no_basis() {
    let fonts = TerminalFonts::default();
    let mut atlas = nearly_full_atlas(&fonts);
    let (mut buffers, mut state) = buffers_and_state();
    let basis = basis_at((2, 1), &atlas, 24);
    state
        .upload(&mut atlas, &mut buffers, &row_of("MA"), &fonts, basis)
        .expect("both buffers exist");
    assert_eq!(atlas.restarts, 1);
    assert_eq!(state.uploaded, None);
}

/// Asserts that an upload with a missing buffer fails before touching
/// the atlas or the glyph table, and forgets the recorded build.
///
/// Case: a pane's cell buffer asset is gone when its next frame, which
/// also resizes the pane and shows new glyphs, arrives.
#[test]
fn an_upload_with_a_missing_buffer_forgets_the_recorded_build() {
    let fonts = TerminalFonts::default();
    let mut atlas = GlyphAtlas::default();
    let (mut buffers, mut state) = buffers_and_state();
    let built = basis_at((1, 1), &atlas, 16);
    state
        .upload(&mut atlas, &mut buffers, &row_of("x"), &fonts, built)
        .expect("both buffers exist");
    assert!(buffers.remove(&state.cells_buffer).is_some());
    let generation = atlas.generation;
    let glyphs = state.glyph_index_map.clone();

    let resized = basis_at((2, 1), &atlas, 16);
    let result = state.upload(&mut atlas, &mut buffers, &row_of("yz"), &fonts, resized);

    assert!(matches!(result, Err(RendererError::MissingShaderBuffer)));
    assert_eq!(atlas.generation, generation);
    assert_eq!(state.glyph_index_map, glyphs);
    assert_eq!(state.uploaded, None);
}

/// Asserts that an upload rewrites the glyph buffer only when its glyph
/// table was rebuilt or gained a glyph.
///
/// Case: a pane receives output that reuses only glyphs it already
/// shows, and then output that introduces a new one.
#[test]
fn an_upload_rewrites_the_glyph_buffer_only_when_the_table_changed() {
    let fonts = TerminalFonts::default();
    let mut atlas = GlyphAtlas::default();
    let (mut buffers, mut state) = buffers_and_state();
    let basis = basis_at((2, 1), &atlas, 16);
    state
        .upload(&mut atlas, &mut buffers, &row_of("ab"), &fonts, basis)
        .expect("both buffers exist");
    buffers
        .get_mut(&state.glyphs_buffer)
        .expect("the glyph buffer exists")
        .data = None;

    state
        .upload(&mut atlas, &mut buffers, &row_of("ba"), &fonts, basis)
        .expect("both buffers exist");
    let encoded_glyphs = |buffers: &Assets<ShaderBuffer>, state: &TerminalMaterialState| {
        buffers
            .get(&state.glyphs_buffer)
            .and_then(|buffer| buffer.data.clone())
    };
    assert_eq!(
        encoded_glyphs(&buffers, &state),
        None,
        "known glyphs leave the glyph buffer unwritten"
    );

    state
        .upload(&mut atlas, &mut buffers, &row_of("bc"), &fonts, basis)
        .expect("both buffers exist");
    let mut expected = ShaderBuffer::default();
    expected.set_data(&state.cpu_glyphs);
    assert_eq!(encoded_glyphs(&buffers, &state), expected.data);
}

/// Asserts that a pane with no visible cells uploads one-element cell
/// and glyph buffers rather than empty ones.
///
/// Case: a pane exists before its first frame reports a size, so its
/// view is still 0 by 0.
#[test]
fn a_zero_sized_pane_uploads_one_element_buffers() {
    let fonts = TerminalFonts::default();
    let mut atlas = GlyphAtlas::default();
    let (mut buffers, mut state) = buffers_and_state();
    let basis = basis_at((0, 0), &atlas, 16);
    state
        .upload(
            &mut atlas,
            &mut buffers,
            &TerminalCells::default(),
            &fonts,
            basis,
        )
        .expect("both buffers exist");
    assert_eq!(state.cpu_cells.len(), 1);
    assert_eq!(state.cpu_glyphs.len(), 1);
    let encoded = buffers
        .get(&state.cells_buffer)
        .and_then(|buffer| buffer.data.as_ref());
    assert!(encoded.is_some_and(|data| !data.is_empty()));
}
