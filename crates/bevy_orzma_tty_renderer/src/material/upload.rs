//! Per-pane cell upload: rebuilds each terminal's cell and glyph buffers
//! when its cells, its size, the glyph atlas or the font size changed.

use crate::{
    glyph::{
        atlas::GlyphAtlas,
        font::{
            CellMetrics, FontFace, GlyphKey, TerminalCellMetricsResource, TerminalFontSize,
            TerminalFonts, physical_font_size,
        },
    },
    material::{
        GpuCell, GpuGlyph, MaterialStage, STYLE_WIDE_RIGHT_HALF, TerminalUiMaterial, pack_linear,
    },
    schema::{
        Color as CellColor, GridCell, GridSlot, HyperlinkId, Palette, Style, TerminalCells,
        TerminalView,
    },
};
use bevy::{
    platform::collections::HashMap, prelude::*, render::storage::ShaderBuffer,
    window::PrimaryWindow,
};

/// Registers the per-pane cell upload.
pub(crate) struct CellUploadPlugin;

impl Plugin for CellUploadPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PostUpdate,
            update_terminal_material.in_set(MaterialStage::Upload),
        );
    }
}

/// CPU-side cache mirroring what the GPU sees this frame.
#[derive(Component)]
pub(crate) struct TerminalMaterialState {
    pub glyph_index_map: HashMap<GlyphKey, u32>,
    pub cpu_cells: Vec<GpuCell>,
    pub cpu_glyphs: Vec<GpuGlyph>,
    pub last_atlas_generation: u64,
    /// Set from [`crate::schema::TerminalCells`]'s change detection and
    /// cleared only once the rebuild actually uploads, so it stays set
    /// across a frame whose rebuild bails out.
    pub grid_dirty: bool,
    pub last_grid_dims: (u16, u16),
    /// Last physical font size used for glyph rasterization; `0` before
    /// the entity's first rebuild.
    pub last_phys_font_size: u16,
    /// Cached output of `TerminalFonts::cell_metrics_px(last_phys_font_size)`.
    pub cached_metrics: Option<CellMetrics>,
    pub initialized: bool,
}

impl TerminalMaterialState {
    /// Resets all glyph-cache state and marks the grid dirty, so the next
    /// `update_terminal_material` invocation fully reuploads the atlas
    /// LUT, glyph rects, and atlas generation marker.
    ///
    /// It leaves `last_phys_font_size`, `cpu_cells`, and `initialized`
    /// untouched; the caller writes `last_phys_font_size` itself after
    /// invalidating.
    pub(crate) fn invalidate_all(&mut self) {
        self.glyph_index_map.clear();
        self.cpu_glyphs.clear();
        self.last_atlas_generation = 0;
        self.grid_dirty = true;
        self.cached_metrics = None;
    }
}

fn update_terminal_material(
    mut atlas: ResMut<GlyphAtlas>,
    mut buffers: ResMut<Assets<ShaderBuffer>>,
    mut terminals: Query<(
        &MaterialNode<TerminalUiMaterial>,
        &mut TerminalMaterialState,
        Ref<TerminalCells>,
        // NOTE: `view` is taken as a plain `&`, never `Ref`. Latching
        //       `view.is_changed()` into `grid_dirty` would make every cursor
        //       move, selection drag and IME toggle rebuild and re-upload the
        //       whole cell SSBO again — the defect the view/cells split removed.
        &TerminalView,
    )>,
    mut cell_metrics_res: ResMut<TerminalCellMetricsResource>,
    materials: Res<Assets<TerminalUiMaterial>>,
    fonts: Res<TerminalFonts>,
    font_size: Res<TerminalFontSize>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let dpr = windows.single().ok().map(|window| window.scale_factor());
    for (handle, mut state, cells, view) in terminals.iter_mut() {
        // NOTE: Latch the cells' change signal before the bail-out below.
        // Bevy clears it once this system has run, so cells written on a
        // frame that skips the upload would otherwise never reach the GPU.
        state.grid_dirty |= cells.is_changed();
        let Some(dpr) = dpr else {
            continue;
        };
        let phys_font_size = physical_font_size(font_size.0, dpr);
        let atlas_invalidated = atlas.generation != state.last_atlas_generation;
        let dims_changed = (view.cols, view.rows) != state.last_grid_dims;
        let grid_changed = state.grid_dirty;
        let phys_size_changed = phys_font_size != state.last_phys_font_size;

        let needs_rebuild = !state.initialized
            || grid_changed
            || atlas_invalidated
            || dims_changed
            || phys_size_changed;

        resolve_metrics(
            &mut state,
            &mut cell_metrics_res,
            &fonts,
            phys_font_size,
            phys_size_changed,
            atlas_invalidated,
        );

        let Some((cells_handle, glyphs_handle)) = materials
            .get(&handle.0)
            .map(|m| (m.cells.clone(), m.glyphs.clone()))
        else {
            continue;
        };

        if needs_rebuild {
            upload_cells(
                &mut state,
                &mut atlas,
                &mut buffers,
                &cells,
                &fonts,
                (&cells_handle, &glyphs_handle),
                phys_font_size,
                (view.cols, view.rows),
            );
        }
    }
}

/// Resolves the cell metrics for `phys_font_size`, clearing the glyph
/// caches first when `phys_size_changed` or `atlas_invalidated` is set,
/// and refreshing the shared cell-metrics resource.
///
/// A `phys_size_changed` resolve also records `phys_font_size` as the
/// state's last physical size, so the next frame reports no change.
fn resolve_metrics(
    state: &mut TerminalMaterialState,
    cell_metrics: &mut TerminalCellMetricsResource,
    fonts: &TerminalFonts,
    phys_font_size: u16,
    phys_size_changed: bool,
    atlas_invalidated: bool,
) -> CellMetrics {
    if phys_size_changed {
        state.invalidate_all();
        state.last_phys_font_size = phys_font_size;
    }

    // NOTE: atlas.generation can advance during this very system (via
    //       get_or_insert in rebuild_cells), and a generation jump means
    //       the atlas pixel buffer was wiped — every cached glyph index
    //       in cpu_cells is now stale and would resolve to garbage
    //       texels. Clearing the LUT here forces a full rerasterization
    //       on the rebuild path.
    if atlas_invalidated {
        state.glyph_index_map.clear();
        state.cpu_glyphs.clear();
    }

    let metrics = if let Some(cached) = state.cached_metrics {
        cached
    } else {
        let m = fonts.cell_metrics_px(phys_font_size);
        state.cached_metrics = Some(m);
        m
    };

    // NOTE: Write the metrics back to TerminalCellMetricsResource so
    //       gui-side resize_terminals_to_node reads DPR-adjusted phys
    //       values on the next frame. The OR condition also catches
    //       the case where the Resource was reset externally (e.g.
    //       hot-reload) even if our local state matches.
    if phys_size_changed || cell_metrics.phys_font_size != phys_font_size {
        *cell_metrics = TerminalCellMetricsResource {
            metrics,
            phys_font_size,
        };
    }

    metrics
}

/// Rebuilds one terminal's cell and glyph buffers and uploads both,
/// then records the atlas generation and grid dimensions the upload was
/// built from.
///
/// `handles` is `(cells, glyphs)`. `dims` is `(cols, rows)` in cells. A
/// zero in either axis uploads the one-element dummy buffers wgpu
/// requires instead of an empty one.
///
/// Rows and columns of `cells` outside `dims` are ignored, and a slot
/// `cells` does not cover keeps the default cell.
fn upload_cells(
    state: &mut TerminalMaterialState,
    atlas: &mut GlyphAtlas,
    buffers: &mut Assets<ShaderBuffer>,
    cells: &TerminalCells,
    fonts: &TerminalFonts,
    handles: (&Handle<ShaderBuffer>, &Handle<ShaderBuffer>),
    phys_font_size: u16,
    dims: (u16, u16),
) {
    let (cols, rows) = (u32::from(dims.0), u32::from(dims.1));
    let (cells_handle, glyphs_handle) = handles;

    let cell_count = (cols * rows) as usize;
    state.cpu_cells.clear();
    state.cpu_cells.resize(cell_count, GpuCell::default());

    if cols > 0 && rows > 0 {
        rebuild_cells(state, atlas, cells, fonts, phys_font_size, (cols, rows));
    }

    if state.cpu_cells.is_empty() {
        state.cpu_cells.push(GpuCell::default());
    }
    if state.cpu_glyphs.is_empty() {
        state.cpu_glyphs.push(GpuGlyph::default());
    }

    if let Some(mut buf) = buffers.get_mut(cells_handle) {
        buf.set_data(&state.cpu_cells);
    }
    if let Some(mut buf) = buffers.get_mut(glyphs_handle) {
        buf.set_data(&state.cpu_glyphs);
    }

    state.last_atlas_generation = atlas.generation;
    state.grid_dirty = false;
    state.last_grid_dims = dims;
    state.initialized = true;
}

fn rebuild_cells(
    state: &mut TerminalMaterialState,
    atlas: &mut GlyphAtlas,
    cells: &TerminalCells,
    fonts: &TerminalFonts,
    phys_font_size: u16,
    dims: (u32, u32),
) {
    let restarts = atlas.restarts;
    fill_cells(state, atlas, cells, fonts, phys_font_size, dims);
    if atlas.restarts == restarts {
        return;
    }
    // NOTE: A restart during the pass wiped the texels every index
    // resolved before it points at; one more pass re-resolves them
    // against the restarted atlas. A second restart means the grid's
    // glyph set does not fit the atlas at all, so that pass is final.
    state.glyph_index_map.clear();
    state.cpu_glyphs.clear();
    fill_cells(state, atlas, cells, fonts, phys_font_size, dims);
}

/// Writes every visible cell's glyph index, color and style into the CPU
/// cell table, resolving each glyph through the atlas as it goes.
///
/// `dims` is `(cols, rows)` in cells. A row or column of `cells` outside
/// it is skipped without resolving its glyphs.
fn fill_cells(
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
        for (col, slot) in row.iter().enumerate().take(cols as usize) {
            let col = col as u32;
            let target = (row_idx as u32 * cols + col) as usize;
            match slot {
                GridSlot::Empty => left_half = None,
                GridSlot::Cell(cell) => {
                    let gpu = GpuCell {
                        glyph_index: resolve_glyph_index(cell, state, fonts, atlas, phys_font_size),
                        fg_packed: packed_palette.cell_fg(cell.fg),
                        bg_packed: packed_palette.cell_bg(cell.bg),
                        style_flags: u32::from(
                            cell.style | style_from_combining_marks(&cell.text).bits(),
                        ),
                        hyperlink_id: cell.hyperlink.map_or(0, HyperlinkId::get),
                    };
                    if let Some(target) = state.cpu_cells.get_mut(target) {
                        *target = gpu;
                    }
                    left_half = Some(gpu);
                }
                // NOTE: For width=2 (CJK / wide) cells we ALSO populate the
                //       right-half slot with the same glyph_index + fg + bg
                //       and set STYLE_WIDE_RIGHT_HALF. The shader uses the
                //       bit to anchor the wide glyph to the left-half cell's
                //       origin (`in_cell_px_eff = in_cell_px + vec2(cell_pitch_px.x, 0)`),
                //       rendering a continuous wide glyph across both cells.
                //       Without this, the right half stays at GpuCell::default
                //       (bg=0 transparent, glyph_index=GLYPH_NONE) and CJK
                //       characters render as half-glyphs with black gaps.
                GridSlot::WideTrailer => {
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

/// Promotes the combining marks in a cell's text that stand for lines to
/// the `Style` underline and strike flags so the shader paints them.
fn style_from_combining_marks(text: &str) -> Style {
    if text.is_ascii() {
        return Style::empty();
    }
    text.chars()
        .filter_map(line_style)
        .fold(Style::empty(), |acc, s| acc | s)
}

/// The marks of a cell's text that are composed onto its glyph: every
/// `char` after the first, except the marks [`line_style`] maps to a line.
fn composable_marks(text: &str) -> impl Iterator<Item = char> + '_ {
    text.chars().skip(1).filter(|c| line_style(*c).is_none())
}

fn resolve_glyph_index(
    cell: &GridCell,
    state: &mut TerminalMaterialState,
    fonts: &TerminalFonts,
    atlas: &mut GlyphAtlas,
    phys_font_size: u16,
) -> u32 {
    if cell.is_blank() {
        return u32::MAX;
    }
    let codepoint = cell.text.chars().next().map(|c| c as u32).unwrap_or(0);
    if codepoint == 0 || codepoint == 0x20 {
        return u32::MAX;
    }
    let face = FontFace::from_style(cell.style);
    let key =
        GlyphKey::new(face, codepoint, phys_font_size).with_marks(composable_marks(&cell.text));
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

/// The transparent cell-background packing (`alpha == 0`) the shader
/// treats as "terminal default background".
const TRANSPARENT_BG: u32 = 0;

/// The grid palette pre-packed to the shader's linear `u32` encoding.
struct PackedPalette {
    indexed: [u32; 256],
    foreground: u32,
    background: u32,
}

impl PackedPalette {
    /// Packs each color slot of `palette` once.
    fn build(palette: &Palette) -> Self {
        Self {
            indexed: palette.indexed.map(pack_linear),
            foreground: pack_linear(palette.foreground),
            background: pack_linear(palette.background),
        }
    }

    /// Packs a cell foreground, resolving symbolic colors to their
    /// palette slot.
    //
    // NOTE: The variant-to-slot mapping mirrors `Palette::resolve` in
    //       `orzma_vt`, pre-packed here for the per-cell hot path; a
    //       change to either mapping must be applied to both, or
    //       symbolic colors silently diverge between producers.
    fn cell_fg(&self, color: CellColor) -> u32 {
        match color {
            CellColor::DefaultForeground => self.foreground,
            CellColor::DefaultBackground => self.background,
            CellColor::Indexed(index) => self.indexed[usize::from(index)],
            CellColor::Rgb(rgb) => pack_linear(rgb),
        }
    }

    /// Packs a cell background.
    ///
    /// # Invariants
    ///
    /// `DefaultBackground` packs [`TRANSPARENT_BG`], never the opaque
    /// palette background, so an explicit RGB equal to that background
    /// stays distinguishable from the default.
    fn cell_bg(&self, color: CellColor) -> u32 {
        match color {
            CellColor::DefaultBackground => TRANSPARENT_BG,
            other => self.cell_fg(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glyph::font::{CellMetrics, FontFace, GlyphKey};

    fn cell_with_link(text: &str, link: Option<u32>) -> GridCell {
        use crate::schema::{Color as CellColor, HyperlinkId};
        GridCell {
            text: text.to_string(),
            fg: CellColor::DefaultForeground,
            bg: CellColor::DefaultBackground,
            style: 0,
            hyperlink: link.map(|id| HyperlinkId::new(id).expect("nonzero")),
        }
    }

    /// Returns the observable payload of each GPU slot as
    /// `(glyph_index, fg, bg, style_flags, hyperlink_id)`.
    fn gpu_cell_fingerprint(cells: &[GpuCell]) -> Vec<(u32, u32, u32, u32, u32)> {
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

    /// Builds a state whose cell buffer is sized for `cell_count` slots.
    fn state_for(cell_count: usize) -> TerminalMaterialState {
        use bevy::platform::collections::HashMap;
        TerminalMaterialState {
            glyph_index_map: HashMap::new(),
            cpu_cells: vec![GpuCell::default(); cell_count],
            cpu_glyphs: Vec::new(),
            last_atlas_generation: 0,
            grid_dirty: true,
            last_grid_dims: (0, 0),
            last_phys_font_size: 0,
            cached_metrics: None,
            initialized: false,
        }
    }

    fn uploaded(
        state: &mut TerminalMaterialState,
        buffers: &mut Assets<ShaderBuffer>,
        handles: (&Handle<ShaderBuffer>, &Handle<ShaderBuffer>),
        cells: &TerminalCells,
        dims: (u16, u16),
    ) {
        let mut atlas = GlyphAtlas::default();
        let fonts = TerminalFonts::default();
        upload_cells(state, &mut atlas, buffers, cells, &fonts, handles, 16, dims);
    }

    fn grid_of(rows: usize, cols: usize) -> TerminalCells {
        TerminalCells {
            cells: vec![vec![GridSlot::Cell(cell_with_link("x", None)); cols]; rows],
            ..Default::default()
        }
    }

    fn hyperlink_ids(cells: &[GpuCell]) -> Vec<u32> {
        cells.iter().map(|cell| cell.hyperlink_id).collect()
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
    fn upload_cells_clips_a_grid_that_disagrees_with_its_dims() {
        let mut buffers = Assets::<ShaderBuffer>::default();
        let cells_handle = buffers.add(ShaderBuffer::default());
        let glyphs_handle = buffers.add(ShaderBuffer::default());
        let linked = |id| GridSlot::Cell(cell_with_link("x", Some(id)));
        let larger = TerminalCells {
            cells: vec![
                vec![linked(1), linked(2), linked(3)],
                vec![GridSlot::Empty, linked(4)],
                vec![linked(5)],
                vec![GridSlot::Cell(cell_with_link("y", Some(6)))],
            ],
            ..Default::default()
        };
        let smaller = TerminalCells {
            cells: vec![vec![linked(7)]],
            ..Default::default()
        };
        let untouched = gpu_cell_fingerprint(&[GpuCell::default()])[0];
        let mut state = state_for(0);

        uploaded(
            &mut state,
            &mut buffers,
            (&cells_handle, &glyphs_handle),
            &larger,
            (2, 3),
        );
        assert_eq!(hyperlink_ids(&state.cpu_cells), [1, 2, 0, 4, 5, 0]);
        let fingerprint = gpu_cell_fingerprint(&state.cpu_cells);
        assert_eq!(fingerprint[2], untouched);
        assert_eq!(fingerprint[5], untouched);
        assert_eq!(
            state.cpu_glyphs.len(),
            1,
            "the row outside the dimensions resolves no glyph"
        );

        uploaded(
            &mut state,
            &mut buffers,
            (&cells_handle, &glyphs_handle),
            &smaller,
            (2, 3),
        );
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
    fn upload_cells_keeps_its_cpu_cell_table_across_uploads() {
        let mut buffers = Assets::<ShaderBuffer>::default();
        let cells_handle = buffers.add(ShaderBuffer::default());
        let glyphs_handle = buffers.add(ShaderBuffer::default());
        let painted = grid_of(2, 2);
        let mut blanked = grid_of(2, 2);
        blanked.cells[1][1] = GridSlot::Empty;
        let mut state = state_for(0);

        for cells in [&painted, &blanked] {
            uploaded(
                &mut state,
                &mut buffers,
                (&cells_handle, &glyphs_handle),
                cells,
                (2, 2),
            );
            assert_eq!(state.cpu_cells.len(), 4);
            let mut expected = ShaderBuffer::default();
            expected.set_data(&state.cpu_cells);
            let encoded = buffers
                .get(&cells_handle)
                .and_then(|buffer| buffer.data.as_ref());
            assert_eq!(encoded, expected.data.as_ref());
        }
        let untouched = gpu_cell_fingerprint(&[GpuCell::default()])[0];
        assert_eq!(gpu_cell_fingerprint(&state.cpu_cells)[3], untouched);
    }

    /// Asserts the GPU slots a row of a wide char, a combining mark and a
    /// linked cell produces, pinning the payload of every slot including
    /// the wide char's right half.
    ///
    /// Case: a file listing hyperlinks a CJK filename so it opens on
    /// click, while an accented latin suffix typed right after it stays
    /// plain, unlinked text.
    #[test]
    fn rebuild_cells_pins_wide_combining_and_linked_slots() {
        let wide = cell_with_link("あ", Some(3));
        let combining = cell_with_link("e\u{0332}", None);
        let plain = cell_with_link("z", None);
        let cells = TerminalCells {
            cells: vec![vec![
                GridSlot::Cell(wide),
                GridSlot::WideTrailer,
                GridSlot::Cell(combining),
                GridSlot::Cell(plain),
            ]],
            ..Default::default()
        };
        let mut state = state_for(4);
        let mut atlas = GlyphAtlas::default();
        let fonts = TerminalFonts::default();

        rebuild_cells(&mut state, &mut atlas, &cells, &fonts, 16, (4, 1));

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

    /// Asserts that a linked cell's wire id reaches its GPU slot while
    /// an unlinked cell's slot keeps the 0 sentinel.
    ///
    /// Case: a row mixes OSC 8 linked text with plain text.
    #[test]
    fn rebuild_cells_writes_hyperlink_id_when_present() {
        let linked = cell_with_link("x", Some(7));
        let unlinked = cell_with_link("y", None);
        let cells = TerminalCells {
            cells: vec![vec![GridSlot::Cell(linked), GridSlot::Cell(unlinked)]],
            ..Default::default()
        };
        let mut state = state_for(2);
        let mut atlas = GlyphAtlas::default();
        let fonts = TerminalFonts::default();

        rebuild_cells(&mut state, &mut atlas, &cells, &fonts, 16, (2, 1));

        assert_eq!(state.cpu_cells[0].hyperlink_id, 7);
        assert_eq!(state.cpu_cells[1].hyperlink_id, 0);
    }

    /// Asserts that a cell resolved before a mid-rebuild atlas restart is
    /// re-resolved against the restarted atlas rather than keeping a
    /// glyph index into the rect the restart evicted.
    ///
    /// Case: a row's first glyph is already cached in the atlas, and its
    /// second glyph overflows a nearly full atlas mid-rebuild.
    #[test]
    fn rebuild_cells_survives_an_atlas_restart_mid_pass() {
        let m = cell_with_link("M", None);
        let a = cell_with_link("A", None);
        let cells = TerminalCells {
            cells: vec![vec![GridSlot::Cell(m), GridSlot::Cell(a)]],
            ..Default::default()
        };
        let mut state = state_for(2);
        let mut atlas = GlyphAtlas::new(32, 24);
        let fonts = TerminalFonts::default();
        // Pack 'M' (12x18) then 'W' (14x18) onto the first shelf so it
        // sits at x=26, leaving no room for 'A' (13x18) beside them and
        // no room below for its height either — resolving 'A' forces the
        // single restart this test exercises, evicting the 'M' rect cell
        // 0 already resolved against.
        atlas
            .get_or_insert(GlyphKey::new(FontFace::Regular, u32::from('M'), 24), &fonts)
            .expect("'M' rasterizes");
        atlas
            .get_or_insert(GlyphKey::new(FontFace::Regular, u32::from('W'), 24), &fonts)
            .expect("'W' rasterizes");
        assert_eq!(
            atlas.restarts, 0,
            "the filler glyphs must not restart the atlas"
        );

        rebuild_cells(&mut state, &mut atlas, &cells, &fonts, 24, (2, 1));

        for (col, ch) in [(0usize, 'M'), (1usize, 'A')] {
            let glyph_index = state.cpu_cells[col].glyph_index;
            let glyph = state.cpu_glyphs[glyph_index as usize];
            let key = GlyphKey::new(FontFace::Regular, u32::from(ch), 24);
            let rect = atlas.glyphs[&key];
            assert_eq!(
                glyph.uv_min,
                Vec2::new(rect.u as f32, rect.v as f32),
                "cell {col} ({ch:?}) glyph index must point at the restarted atlas's rect"
            );
        }
        assert_eq!(atlas.restarts, 1);
    }

    /// Asserts that the underline-like combining marks promote to
    /// `Style::UNDERLINE`, the long stroke overlay to `Style::STRIKE`, and
    /// any other text to no flags.
    ///
    /// Case: a program decorates text with combining low lines and stroke
    /// overlays next to plain ASCII and accented text.
    #[test]
    fn combining_marks_promote_to_underline_and_strike() {
        assert_eq!(style_from_combining_marks("a"), Style::empty());
        assert_eq!(style_from_combining_marks("e\u{0301}"), Style::empty());
        for mark in ['\u{0331}', '\u{0332}', '\u{0333}'] {
            assert_eq!(
                style_from_combining_marks(&format!("a{mark}")),
                Style::UNDERLINE
            );
        }
        assert_eq!(style_from_combining_marks("a\u{0336}"), Style::STRIKE);
        assert_eq!(
            style_from_combining_marks("a\u{0332}\u{0336}"),
            Style::UNDERLINE | Style::STRIKE
        );
    }

    /// Asserts that fg and bg packing resolve symbolic colors through
    /// the live palette, and that the default background packs the
    /// transparent sentinel instead of the palette value.
    ///
    /// Case: a palette whose default foreground and indexed slot 1
    /// already hold custom colors packs cells for a webview overlay
    /// mounted behind default-background cells.
    #[test]
    fn cell_packing_resolves_through_the_live_palette() {
        use crate::schema::{Color as CellColor, Palette, Rgb};
        let mut palette = Palette {
            foreground: Rgb {
                r: 10,
                g: 20,
                b: 30,
            },
            ..Palette::default()
        };
        palette.indexed[1] = Rgb {
            r: 40,
            g: 50,
            b: 60,
        };
        let packed = PackedPalette::build(&palette);
        assert_eq!(
            packed.cell_fg(CellColor::DefaultForeground),
            pack_linear(Rgb {
                r: 10,
                g: 20,
                b: 30,
            })
        );
        assert_eq!(
            packed.cell_fg(CellColor::Indexed(1)),
            pack_linear(Rgb {
                r: 40,
                g: 50,
                b: 60,
            })
        );
        assert_eq!(packed.cell_bg(CellColor::DefaultBackground), TRANSPARENT_BG);
        assert_ne!(
            packed.cell_bg(CellColor::Rgb(palette.background)),
            TRANSPARENT_BG,
            "an explicit RGB equal to the palette background must stay opaque"
        );
    }

    /// Asserts that the marks the shader draws as lines are left out of
    /// the composable set, and that the base glyph is never one of them.
    ///
    /// Case: a cell holds `e` with an acute accent, a combining low line,
    /// a long stroke overlay and a tilde.
    #[test]
    fn composable_marks_skip_the_base_and_the_line_marks() {
        assert_eq!(
            composable_marks("e\u{0301}\u{0332}\u{0336}\u{0303}").collect::<Vec<_>>(),
            ['\u{0301}', '\u{0303}']
        );
        assert_eq!(composable_marks("a").count(), 0);
        assert_eq!(composable_marks("").count(), 0);
    }

    fn populated_state() -> TerminalMaterialState {
        let mut state = TerminalMaterialState {
            glyph_index_map: HashMap::new(),
            cpu_cells: Vec::new(),
            cpu_glyphs: vec![GpuGlyph::default(), GpuGlyph::default()],
            last_atlas_generation: 42,
            grid_dirty: false,
            last_grid_dims: (80, 24),
            last_phys_font_size: 24,
            cached_metrics: Some(CellMetrics {
                advance_phys: 5.5,
                line_height_phys: 14.4,
                ascent_phys: 10.0,
                descent_phys: 2.4,
                underline_position_phys: -1.5,
                underline_thickness_phys: 1.0,
                max_overflow_phys: 0.0,
            }),
            initialized: true,
        };
        state
            .glyph_index_map
            .insert(GlyphKey::new(FontFace::Regular, 'A' as u32, 24), 7);
        state
    }

    #[test]
    fn invalidate_all_clears_lut_and_atlas_markers() {
        let mut state = populated_state();
        state.invalidate_all();
        assert!(state.glyph_index_map.is_empty());
        assert!(state.cpu_glyphs.is_empty());
        assert_eq!(state.last_atlas_generation, 0);
        assert!(state.grid_dirty);
        assert!(state.cached_metrics.is_none());
    }

    #[test]
    fn invalidate_all_preserves_phys_font_size() {
        let mut state = populated_state();
        state.invalidate_all();
        assert_eq!(state.last_phys_font_size, 24);
        assert!(state.initialized);
    }
}
