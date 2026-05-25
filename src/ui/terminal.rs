//! Post-setup for Terminal Activity hosts. After the registry has prepared a
//! stable entity carrying `ActivityHostNode` + `TerminalActivityMarker`, this
//! system spawns a `TerminalBundle` (PTY + VT bridge) and attaches a
//! `TerminalRenderBundle` (renderer-side grid + MaterialNode) exactly once.
//! Failures mark the entity with `TerminalSpawnFailed` so the system does
//! not retry on subsequent frames.

use crate::ui::{TerminalActivityMarker, TerminalSpawnFailed};
use bevy::prelude::*;
use bevy_terminal::{Coalescer, PtyHandle, SpawnOptions, TerminalBundle, TerminalHandle};
use bevy_terminal_renderer::material::TerminalUiMaterial;
use bevy_terminal_renderer::prelude::{TerminalGrid, TerminalRenderBundle};

/// Natural logical-pixel width of one glyph cell, mirrors the constant
/// inside the renderer's `update_terminal_material`. Used here only to
/// estimate the maximum cols that fit in the pane; the GPU side stretches
/// each cell's pitch to `node_size / grid_size` (see
/// `terminal_ui_material.wgsl::cell_pitch_px`) so the grid fills the pane
/// edge-to-edge with zero remainder.
pub(crate) const CELL_W_LOGICAL_PX: f32 = 8.0;
/// Natural logical-pixel height of one glyph cell. See `CELL_W_LOGICAL_PX`.
pub(crate) const CELL_H_LOGICAL_PX: f32 = 16.0;

/// Spawns a `TerminalBundle` and attaches `TerminalRenderBundle` for each
/// freshly-spawned Terminal Activity host. Runs every Update tick but only
/// targets entities that lack `TerminalHandle` and `TerminalSpawnFailed`,
/// so the per-entity work happens exactly once.
#[expect(clippy::type_complexity, reason = "Bevy query filter tuple")]
pub(crate) fn finish_terminal_setup(
    mut commands: Commands,
    hosts: Query<
        Entity,
        (
            With<TerminalActivityMarker>,
            Without<TerminalHandle>,
            Without<TerminalSpawnFailed>,
        ),
    >,
    mut materials: ResMut<Assets<TerminalUiMaterial>>,
) {
    for host in hosts.iter() {
        let opts = SpawnOptions {
            cols: 80,
            rows: 24,
            shell: std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into()),
            cwd: std::env::var_os("HOME").map(std::path::PathBuf::from),
            env: Vec::new(),
        };
        let bundle = match TerminalBundle::spawn(opts) {
            Ok(b) => b,
            Err(e) => {
                tracing::error!(?e, ?host, "TerminalBundle::spawn failed");
                commands.entity(host).insert(TerminalSpawnFailed);
                continue;
            }
        };
        let material_handle = materials.add(TerminalUiMaterial::default());
        commands
            .entity(host)
            .insert((bundle, TerminalRenderBundle::new(material_handle)));
    }
}

/// Resizes each Terminal Activity's PTY / VT grid to match its host UI
/// node's pixel extents so the shader's `grid_size * cell_size_px` always
/// fills the entire pane. Idempotent — no-op when cols/rows are unchanged.
///
/// Runs in `PostUpdate` after `UiSystems::Layout` so `ComputedNode.size`
/// reflects the current frame's layout. Also writes the new `cols`/`rows`
/// directly into `TerminalGrid` so the renderer's `update_terminal_material`
/// (also `PostUpdate`) can rebuild the uniform in the same tick — without
/// this short-circuit the new dimensions would only reach the shader after
/// the next `FrameSnapshot` round-trip through alacritty + observers,
/// adding a visible 1-frame lag at the pane edge during drag.
#[expect(
    clippy::type_complexity,
    reason = "Bevy query: TerminalHandle / PtyHandle / Coalescer / TerminalGrid are independent components"
)]
pub(crate) fn resize_terminals_to_node(
    mut terminals: Query<(
        &ComputedNode,
        &mut TerminalHandle,
        &mut PtyHandle,
        &mut Coalescer,
        &mut TerminalGrid,
    )>,
    windows: Query<&Window>,
) {
    let dpr = windows.single().map(|w| w.scale_factor()).unwrap_or(1.0);
    for (computed, mut handle, mut pty, mut coalescer, mut grid) in terminals.iter_mut() {
        // `ComputedNode.size` is physical pixels; cell sizes are logical px.
        let logical_w = (computed.size.x / dpr).max(0.0);
        let logical_h = (computed.size.y / dpr).max(0.0);
        // Pick the largest cols / rows that fit the pane in natural cell
        // metrics. The shader then stretches each cell's pitch to
        // `node_size / grid_size` so the grid fills the pane exactly —
        // no sub-cell strip on the right or bottom. Glyph bitmaps stay
        // at their native 8x16 logical px (centered within the stretched
        // pitch and pixel-snapped), so the only visible distortion is a
        // sub-pixel change in inter-cell spacing.
        let cols = ((logical_w / CELL_W_LOGICAL_PX).floor() as u16).max(1);
        let rows = ((logical_h / CELL_H_LOGICAL_PX).floor() as u16).max(1);
        let (cur_cols, cur_rows, _) = handle.read_geometry();
        if cur_cols == cols && cur_rows == rows {
            continue;
        }
        if let Err(e) = handle.resize(&mut pty, &mut coalescer, cols, rows) {
            tracing::warn!(?e, cols, rows, "TerminalHandle::resize failed");
            continue;
        }
        // NOTE: load-bearing for zero-lag resize. `update_terminal_material`
        // reads `grid.cols`/`grid.rows` for the `grid_size` uniform, but
        // `TerminalGrid` is normally only refreshed when `apply_snapshot`
        // observes a `FrameSnapshot` trigger emitted later by
        // `check_deadline_flush`. Writing the new geometry here makes the
        // shader see the correct grid size on the very next render this
        // frame; the snapshot will still arrive a frame later and refresh
        // the cell contents, which appear as activity-bg empty cells in the
        // meantime — visually indistinguishable from the rest of the empty
        // grid.
        grid.cols = cols;
        grid.rows = rows;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::asset::AssetPlugin;
    use bevy::image::ImagePlugin;
    use bevy::render::storage::ShaderStorageBuffer;

    fn make_test_app() -> App {
        // NOTE: `TerminalRendererPlugin` calls `load_internal_asset!` which
        // requires the full render plugin chain. For these headless tests we
        // only need `Assets<TerminalUiMaterial>` to exist so the system's
        // `ResMut<Assets<...>>` parameter resolves. Manually `init_asset`
        // the material and its storage-buffer dependency instead.
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(AssetPlugin::default())
            .add_plugins(ImagePlugin::default())
            .init_asset::<TerminalUiMaterial>()
            .init_asset::<ShaderStorageBuffer>();
        app
    }

    #[test]
    fn skips_entities_without_terminal_marker() {
        let mut app = make_test_app();
        app.add_systems(Update, finish_terminal_setup);
        let host = app.world_mut().spawn_empty().id();
        app.update();
        assert!(
            app.world().get::<TerminalHandle>(host).is_none(),
            "entity without TerminalActivityMarker must not receive TerminalHandle"
        );
    }

    #[test]
    fn marks_host_failed_when_spawn_errors() {
        let _guard = crate::configs::env_guard();
        // SAFETY: env_guard is held; we mutate SHELL to force
        // TerminalBundle::spawn (specifically spawn_command) to fail.
        unsafe {
            std::env::set_var("SHELL", "/nonexistent-binary-for-test");
        }
        let mut app = make_test_app();
        app.add_systems(Update, finish_terminal_setup);
        let host = app.world_mut().spawn(TerminalActivityMarker).id();
        app.update();
        // SAFETY: env_guard is still held; restore SHELL state so concurrent
        // tests don't see a dirty env.
        unsafe {
            std::env::remove_var("SHELL");
        }
        assert!(
            app.world().get::<TerminalSpawnFailed>(host).is_some(),
            "spawn failure must mark the host with TerminalSpawnFailed"
        );
        assert!(
            app.world().get::<TerminalHandle>(host).is_none(),
            "spawn failure must not leave a TerminalHandle on the host"
        );
    }
}
