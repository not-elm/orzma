//! Per-pane cell upload: rebuilds each terminal's cell and glyph buffers
//! when its cells, its size, the glyph atlas or the font size changed.

use crate::{
    error::{RendererError, RendererResult},
    glyph::{
        atlas::GlyphAtlas,
        font::{FontFace, GlyphKey, TerminalCellMetricsResource, TerminalFonts},
    },
    material::{GpuCell, GpuGlyph, MaterialStage, STYLE_WIDE_RIGHT_HALF, pack_linear},
    schema::{
        Color as CellColor, GridCell, GridSlot, HyperlinkId, Palette, Style, TerminalCells,
        TerminalView,
    },
};
use bevy::{platform::collections::HashMap, prelude::*, render::storage::ShaderBuffer};

/// Registers the per-pane cell upload.
pub(crate) struct CellUploadPlugin;

impl Plugin for CellUploadPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PostUpdate,
            upload_terminal_cells
                .in_set(MaterialStage::Upload)
                .run_if(resource_exists::<TerminalCellMetricsResource>),
        );
    }
}

/// CPU-side cache mirroring what one pane's cell and glyph buffers hold.
#[derive(Component)]
pub(crate) struct TerminalMaterialState {
    cells_buffer: Handle<ShaderBuffer>,
    glyphs_buffer: Handle<ShaderBuffer>,
    glyph_index_map: HashMap<GlyphKey, u32>,
    cpu_cells: Vec<GpuCell>,
    cpu_glyphs: Vec<GpuGlyph>,
    /// `None` until the buffers hold a valid build: before the first one,
    /// after a failed upload, and after a build during which the atlas
    /// restarted.
    uploaded: Option<UploadBasis>,
}

impl TerminalMaterialState {
    /// A cache that uploads into `cells_buffer` and `glyphs_buffer`, with
    /// nothing built yet.
    pub fn new(cells_buffer: Handle<ShaderBuffer>, glyphs_buffer: Handle<ShaderBuffer>) -> Self {
        Self {
            cells_buffer,
            glyphs_buffer,
            glyph_index_map: HashMap::new(),
            cpu_cells: Vec::new(),
            cpu_glyphs: Vec::new(),
            uploaded: None,
        }
    }

    /// Whether the buffers must be rebuilt for `basis`, given whether the
    /// pane's cells changed since the last run.
    fn needs_upload(&self, basis: UploadBasis, cells_changed: bool) -> bool {
        cells_changed || self.uploaded != Some(basis)
    }

    /// Rebuilds both buffers for `cells` at `basis`, recording `basis` only
    /// when the atlas did not restart during the build.
    ///
    /// The glyph table is kept only when the recorded build has the atlas
    /// restarts and the font size of `basis`. A zero in either axis of
    /// `basis.dims` uploads the one-element buffers wgpu requires instead of
    /// empty ones, and rows and columns of `cells` outside the dimensions
    /// are ignored.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::MissingShaderBuffer`], leaving the atlas and
    /// this cache untouched, when either buffer asset is missing.
    fn upload(
        &mut self,
        atlas: &mut GlyphAtlas,
        buffers: &mut Assets<ShaderBuffer>,
        cells: &TerminalCells,
        fonts: &TerminalFonts,
        basis: UploadBasis,
    ) -> RendererResult {
        if !buffers.contains(&self.cells_buffer) || !buffers.contains(&self.glyphs_buffer) {
            return Err(RendererError::MissingShaderBuffer);
        }
        let keeps_glyphs = self
            .uploaded
            .is_some_and(|built| built.keeps_glyphs_for(basis));
        if !keeps_glyphs {
            self.glyph_index_map.clear();
            self.cpu_glyphs.clear();
        }
        let (cols, rows) = (u32::from(basis.dims.0), u32::from(basis.dims.1));
        self.cpu_cells.clear();
        self.cpu_cells
            .resize((cols * rows) as usize, GpuCell::default());
        if cols > 0 && rows > 0 {
            fill_cells(
                self,
                atlas,
                cells,
                fonts,
                basis.phys_font_size,
                (cols, rows),
            );
        }
        if self.cpu_cells.is_empty() {
            self.cpu_cells.push(GpuCell::default());
        }
        if self.cpu_glyphs.is_empty() {
            self.cpu_glyphs.push(GpuGlyph::default());
        }
        if let Some(mut buffer) = buffers.get_mut(&self.cells_buffer) {
            buffer.set_data(&self.cpu_cells);
        }
        if let Some(mut buffer) = buffers.get_mut(&self.glyphs_buffer) {
            buffer.set_data(&self.cpu_glyphs);
        }
        self.uploaded = (atlas.restarts == basis.atlas_restarts).then_some(basis);
        Ok(())
    }
}

/// What a pane's cell and glyph buffers were built from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct UploadBasis {
    /// The `(cols, rows)` of the pane's `TerminalView`.
    dims: (u16, u16),
    /// The `GlyphAtlas::restarts` every glyph index in the buffers is valid
    /// for.
    atlas_restarts: u64,
    /// The physical font size the glyphs were keyed at.
    phys_font_size: u16,
}

impl UploadBasis {
    /// The basis a pane showing `view` would be built from now.
    fn current(
        view: &TerminalView,
        atlas: &GlyphAtlas,
        metrics: &TerminalCellMetricsResource,
    ) -> Self {
        Self {
            dims: (view.cols, view.rows),
            atlas_restarts: atlas.restarts,
            phys_font_size: metrics.phys_font_size,
        }
    }

    /// Whether a glyph table built at this basis still indexes the atlas
    /// correctly at `other`.
    fn keeps_glyphs_for(self, other: Self) -> bool {
        self.atlas_restarts == other.atlas_restarts && self.phys_font_size == other.phys_font_size
    }
}

/// Rebuilds the buffers of every pane whose cells, size, atlas restart
/// count or font size changed; when a rebuild restarted the atlas, rebuilds
/// once more every pane that restart left stale.
fn upload_terminal_cells(
    mut atlas: ResMut<GlyphAtlas>,
    mut buffers: ResMut<Assets<ShaderBuffer>>,
    mut terminals: Query<(
        Entity,
        &mut TerminalMaterialState,
        Ref<TerminalCells>,
        // NOTE: `view` is read as a plain `&`, never `Ref`: rebuilding when
        //       the view changed would re-upload the whole cell buffer on
        //       every cursor move, selection drag and IME toggle, which
        //       change only the uniforms.
        &TerminalView,
    )>,
    fonts: Res<TerminalFonts>,
    metrics: Res<TerminalCellMetricsResource>,
) {
    let restarts_before = atlas.restarts;
    for (entity, mut state, cells, view) in &mut terminals {
        let basis = UploadBasis::current(view, &atlas, &metrics);
        if state.needs_upload(basis, cells.is_changed()) {
            upload_pane(
                &mut state,
                &mut atlas,
                &mut buffers,
                entity,
                &cells,
                &fonts,
                basis,
            );
        }
    }
    if atlas.restarts == restarts_before {
        return;
    }
    for (entity, mut state, cells, view) in &mut terminals {
        let basis = UploadBasis::current(view, &atlas, &metrics);
        if state.needs_upload(basis, false) {
            upload_pane(
                &mut state,
                &mut atlas,
                &mut buffers,
                entity,
                &cells,
                &fonts,
                basis,
            );
        }
    }
}

/// Uploads one pane; a failure is logged and forgets the recorded build, so
/// the next run rebuilds the pane.
fn upload_pane(
    state: &mut TerminalMaterialState,
    atlas: &mut GlyphAtlas,
    buffers: &mut Assets<ShaderBuffer>,
    entity: Entity,
    cells: &TerminalCells,
    fonts: &TerminalFonts,
    basis: UploadBasis,
) {
    if let Err(err) = state.upload(atlas, buffers, cells, fonts, basis) {
        warn!(terminal = ?entity, %err, "cell upload failed; the next run retries it");
        state.uploaded = None;
    }
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
    use bevy::ecs::change_detection::Tick;

    fn cell_with_link(text: &str, link: Option<u32>) -> GridCell {
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

    fn hyperlink_ids(cells: &[GpuCell]) -> Vec<u32> {
        cells.iter().map(|cell| cell.hyperlink_id).collect()
    }

    fn grid_of(rows: usize, cols: usize) -> TerminalCells {
        TerminalCells {
            cells: vec![vec![GridSlot::Cell(cell_with_link("x", None)); cols]; rows],
            ..Default::default()
        }
    }

    /// One row holding a plain cell for each `char` of `text`.
    fn row_of(text: &str) -> TerminalCells {
        TerminalCells {
            cells: vec![
                text.chars()
                    .map(|ch| GridSlot::Cell(cell_with_link(&ch.to_string(), None)))
                    .collect(),
            ],
            ..Default::default()
        }
    }

    /// A cache sized for `cell_count` default cells, whose buffers are
    /// unused handles.
    fn state_for(cell_count: usize) -> TerminalMaterialState {
        TerminalMaterialState {
            cpu_cells: vec![GpuCell::default(); cell_count],
            ..TerminalMaterialState::new(Handle::default(), Handle::default())
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

    /// A 32x24 atlas whose one shelf already holds 24 px `M` and `W`, so a
    /// 24 px `A` restarts it.
    fn nearly_full_atlas(fonts: &TerminalFonts) -> GlyphAtlas {
        let mut atlas = GlyphAtlas::new(32, 24);
        for ch in ['M', 'W'] {
            atlas
                .get_or_insert(GlyphKey::new(FontFace::Regular, u32::from(ch), 24), fonts)
                .expect("the filler glyph rasterizes");
        }
        assert_eq!(
            atlas.restarts, 0,
            "the filler glyphs must not restart the atlas"
        );
        atlas
    }

    /// The top-left corner of the rect the atlas holds for the plain 24 px
    /// glyph `ch`.
    fn rect_origin(atlas: &GlyphAtlas, ch: char) -> Vec2 {
        let rect = atlas.glyphs[&GlyphKey::new(FontFace::Regular, u32::from(ch), 24)];
        Vec2::new(f32::from(rect.u), f32::from(rect.v))
    }

    /// An app that runs only the cell upload, with `atlas`, 24 px metrics
    /// and no primary window.
    fn upload_app(atlas: GlyphAtlas) -> App {
        let fonts = TerminalFonts::default();
        let mut app = App::new();
        app.add_plugins(CellUploadPlugin)
            .insert_resource(TerminalCellMetricsResource::new(&fonts, 24))
            .insert_resource(fonts)
            .insert_resource(atlas)
            .init_resource::<Assets<ShaderBuffer>>();
        app
    }

    /// Spawns a one-row pane showing `text`, with fresh buffers.
    fn spawn_pane(app: &mut App, text: &str) -> Entity {
        let state = {
            let mut buffers = app.world_mut().resource_mut::<Assets<ShaderBuffer>>();
            let cells = buffers.add(ShaderBuffer::default());
            let glyphs = buffers.add(ShaderBuffer::default());
            TerminalMaterialState::new(cells, glyphs)
        };
        let view = TerminalView {
            cols: u16::try_from(text.chars().count()).expect("a short test row"),
            rows: 1,
            ..Default::default()
        };
        app.world_mut().spawn((state, view, row_of(text))).id()
    }

    fn state_of(app: &App, pane: Entity) -> &TerminalMaterialState {
        app.world()
            .get::<TerminalMaterialState>(pane)
            .expect("the pane's cache")
    }

    fn last_built(app: &App, pane: Entity) -> Tick {
        app.world()
            .entity(pane)
            .get_ref::<TerminalMaterialState>()
            .expect("the pane's cache")
            .last_changed()
    }

    fn set_cells(app: &mut App, pane: Entity, text: &str) {
        *app.world_mut()
            .get_mut::<TerminalCells>(pane)
            .expect("the pane's cells") = row_of(text);
    }

    fn encoded_cells(app: &App, pane: Entity) -> Option<Vec<u8>> {
        let state = state_of(app, pane);
        app.world()
            .resource::<Assets<ShaderBuffer>>()
            .get(&state.cells_buffer)
            .and_then(|buffer| buffer.data.clone())
    }

    #[derive(Resource, Default)]
    struct VisitOrder(Vec<Entity>);

    fn record_visit_order(
        mut order: ResMut<VisitOrder>,
        panes: Query<(
            Entity,
            &TerminalMaterialState,
            &TerminalCells,
            &TerminalView,
        )>,
    ) {
        order.0 = panes.iter().map(|(pane, ..)| pane).collect();
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

        uploaded(&mut state, &mut buffers, &larger, (2, 3));
        assert_eq!(hyperlink_ids(&state.cpu_cells), [1, 2, 0, 4, 5, 0]);
        let fingerprint = gpu_cell_fingerprint(&state.cpu_cells);
        assert_eq!(fingerprint[2], untouched);
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
        blanked.cells[1][1] = GridSlot::Empty;

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
    fn filling_pins_wide_combining_and_linked_slots() {
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

    /// Asserts that a linked cell's wire id reaches its GPU slot while
    /// an unlinked cell's slot keeps the 0 sentinel.
    ///
    /// Case: a row mixes OSC 8 linked text with plain text.
    #[test]
    fn filling_writes_the_hyperlink_id_when_present() {
        let linked = cell_with_link("x", Some(7));
        let unlinked = cell_with_link("y", None);
        let cells = TerminalCells {
            cells: vec![vec![GridSlot::Cell(linked), GridSlot::Cell(unlinked)]],
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
        use crate::schema::Rgb;
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
    /// the atlas, the glyph table or the recorded build.
    ///
    /// Case: a pane's cell buffer asset is gone when its next frame, which
    /// also resizes the pane and shows new glyphs, arrives.
    #[test]
    fn an_upload_with_a_missing_buffer_changes_nothing() {
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
        assert_eq!(state.uploaded, Some(built));
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

    /// Asserts that a change to a pane's cells reaches its cell buffer with
    /// no primary window present.
    ///
    /// Case: output arrives while the window is being re-created, so no
    /// primary window exists for that frame.
    #[test]
    fn a_cell_change_is_uploaded_without_a_primary_window() {
        let mut app = upload_app(GlyphAtlas::default());
        let pane = spawn_pane(&mut app, "ab");
        app.update();
        let before = encoded_cells(&app, pane);

        set_cells(&mut app, pane, "cd");
        app.update();

        let mut expected = ShaderBuffer::default();
        expected.set_data(&state_of(&app, pane).cpu_cells);
        let after = encoded_cells(&app, pane);
        assert_ne!(after, before);
        assert_eq!(after, expected.data);
    }

    /// Asserts that a glyph newly rasterized for one pane, which grows the
    /// atlas without restarting it, leaves another pane unbuilt.
    ///
    /// Case: one pane prints characters it has never shown before while a
    /// second pane sits idle.
    #[test]
    fn a_glyph_added_for_one_pane_leaves_another_pane_unbuilt() {
        let mut app = upload_app(GlyphAtlas::default());
        let idle = spawn_pane(&mut app, "ab");
        let busy = spawn_pane(&mut app, "cd");
        app.update();
        let idle_built = last_built(&app, idle);
        let generation = app.world().resource::<GlyphAtlas>().generation;

        set_cells(&mut app, busy, "ef");
        app.update();

        let atlas = app.world().resource::<GlyphAtlas>();
        assert!(
            atlas.generation > generation,
            "the busy pane rasterized new glyphs"
        );
        assert_eq!(atlas.restarts, 0);
        assert_eq!(last_built(&app, idle), idle_built);
    }

    /// Asserts that when one pane's build restarts the atlas, a pane the
    /// same run visited before the restart is rebuilt within that run
    /// against the restarted atlas.
    ///
    /// Case: two panes share a nearly full atlas, and the second pane prints
    /// a glyph that does not fit.
    #[test]
    fn an_atlas_restart_rebuilds_the_panes_it_left_stale_in_the_same_run() {
        let mut app = upload_app(GlyphAtlas::new(32, 24));
        let idle = spawn_pane(&mut app, "M");
        let busy = spawn_pane(&mut app, "W");
        app.init_resource::<VisitOrder>()
            .add_systems(PostUpdate, record_visit_order.before(MaterialStage::Upload));
        app.update();
        assert_eq!(
            app.world().resource::<VisitOrder>().0,
            [idle, busy],
            "the idle pane is visited first"
        );
        assert_eq!(app.world().resource::<GlyphAtlas>().restarts, 0);

        set_cells(&mut app, busy, "A");
        app.update();

        let atlas = app.world().resource::<GlyphAtlas>();
        assert_eq!(atlas.restarts, 1, "`A` does not fit beside `M` and `W`");
        let idle_state = state_of(&app, idle);
        assert_eq!(
            idle_state.uploaded.map(|built| built.atlas_restarts),
            Some(1)
        );
        let glyph = idle_state.cpu_glyphs[idle_state.cpu_cells[0].glyph_index as usize];
        assert_eq!(glyph.uv_min, rect_origin(atlas, 'M'));
    }

    /// Asserts that a pane whose own build restarted the atlas is rebuilt in
    /// the same run and recorded as built against the restarted atlas.
    ///
    /// Case: a pane prints a row whose second glyph overflows a nearly full
    /// atlas that already caches the first.
    #[test]
    fn a_pane_whose_build_restarted_the_atlas_is_rebuilt_in_the_same_run() {
        let fonts = TerminalFonts::default();
        let mut app = upload_app(nearly_full_atlas(&fonts));
        let pane = spawn_pane(&mut app, "MA");
        app.update();

        let atlas = app.world().resource::<GlyphAtlas>();
        assert_eq!(atlas.restarts, 1);
        let state = state_of(&app, pane);
        assert_eq!(state.uploaded.map(|built| built.atlas_restarts), Some(1));
        for (col, ch) in [(0usize, 'M'), (1usize, 'A')] {
            let glyph = state.cpu_glyphs[state.cpu_cells[col].glyph_index as usize];
            assert_eq!(glyph.uv_min, rect_origin(atlas, ch), "cell {col} ({ch:?})");
        }
    }

    /// Asserts that a pane whose glyphs never fit the atlas together ends
    /// the run unrecorded rather than rebuilding without end.
    ///
    /// Case: a large font zoom leaves a pane showing more distinct glyphs
    /// than the atlas can hold at once.
    #[test]
    fn a_pane_whose_glyphs_never_fit_the_atlas_stays_unrecorded() {
        let mut app = upload_app(GlyphAtlas::new(32, 24));
        let pane = spawn_pane(&mut app, "MAW");
        app.update();
        assert_eq!(state_of(&app, pane).uploaded, None);
        assert_eq!(
            app.world().resource::<GlyphAtlas>().restarts,
            2,
            "the first pass and the one extra pass each restarted the atlas"
        );
    }

    /// Asserts that a pane whose cell buffer is missing is not recorded as
    /// built, and is rebuilt on the first run after the buffer returns.
    ///
    /// Case: a pane's cell buffer asset is gone for a frame while output
    /// keeps arriving.
    #[test]
    fn a_pane_with_a_missing_buffer_is_retried_once_the_buffer_returns() {
        let mut app = upload_app(GlyphAtlas::default());
        let pane = spawn_pane(&mut app, "ab");
        app.update();
        let cells_buffer = state_of(&app, pane).cells_buffer.clone();
        let parked = app
            .world_mut()
            .resource_mut::<Assets<ShaderBuffer>>()
            .remove(&cells_buffer)
            .expect("the cell buffer exists");
        set_cells(&mut app, pane, "cd");
        let generation = app.world().resource::<GlyphAtlas>().generation;
        app.update();
        assert_eq!(state_of(&app, pane).uploaded, None);
        assert_eq!(
            app.world().resource::<GlyphAtlas>().generation,
            generation,
            "the failed upload rasterized nothing"
        );

        app.world_mut()
            .resource_mut::<Assets<ShaderBuffer>>()
            .insert(&cells_buffer, parked)
            .expect("the buffer's id is still live");
        app.update();
        assert!(state_of(&app, pane).uploaded.is_some());
    }

    /// Asserts that a change to a pane's view that keeps its size leaves
    /// the pane unbuilt.
    ///
    /// Case: an IME composition starts in an idle pane, which only hides
    /// the caret.
    #[test]
    fn a_view_change_that_keeps_the_size_rebuilds_nothing() {
        let mut app = upload_app(GlyphAtlas::default());
        let pane = spawn_pane(&mut app, "ab");
        app.update();
        let built = last_built(&app, pane);
        app.world_mut()
            .get_mut::<TerminalView>(pane)
            .expect("the pane's view")
            .suppress_cursor = true;
        app.update();
        assert_eq!(last_built(&app, pane), built);
    }

    /// Asserts that a change of the physical font size rebuilds every pane
    /// in the same run, with glyphs keyed at the new size only.
    ///
    /// Case: the user zooms the font while two panes are open.
    #[test]
    fn a_font_size_change_rebuilds_every_pane_at_the_new_size() {
        let mut app = upload_app(GlyphAtlas::default());
        let panes = [spawn_pane(&mut app, "ab"), spawn_pane(&mut app, "cd")];
        app.update();
        let zoomed = TerminalCellMetricsResource::new(app.world().resource::<TerminalFonts>(), 30);
        app.insert_resource(zoomed);
        app.update();
        for pane in panes {
            let state = state_of(&app, pane);
            assert_eq!(state.uploaded.map(|built| built.phys_font_size), Some(30));
            assert_eq!(state.glyph_index_map.len(), 2);
            assert!(state.glyph_index_map.keys().all(|key| key.size_px == 30));
        }
    }
}
