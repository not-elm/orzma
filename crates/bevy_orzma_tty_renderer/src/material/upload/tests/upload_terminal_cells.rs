//! Tests for the system that rebuilds every pane's buffers when needed.

use super::*;
use bevy::ecs::change_detection::Tick;

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
