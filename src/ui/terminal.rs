//! Post-setup for Terminal Activity hosts. After the registry has prepared a
//! stable entity carrying `ActivityHostNode` + `TerminalActivityMarker`, this
//! system spawns a `TerminalBundle` (PTY + VT bridge) and attaches a
//! `TerminalRenderBundle` (renderer-side grid + MaterialNode) exactly once.
//! Failures mark the entity with `TerminalSpawnFailed` so the system does
//! not retry on subsequent frames.

use crate::extension_manager::ExtensionRegistry;
use crate::system_set::OzmuxSystems;
use crate::ui::{HostActivityEntity, TerminalActivityMarker, TerminalSpawnFailed};
use bevy::prelude::*;
use bevy::ui::UiSystems;
use bevy_terminal::{Coalescer, PtyHandle, SpawnOptions, TerminalBundle, TerminalHandle};
use bevy_terminal_renderer::TerminalCellMetricsResource;
use bevy_terminal_renderer::material::{TerminalMaterialSystems, TerminalUiMaterial};
use bevy_terminal_renderer::prelude::{TerminalGrid, TerminalRenderBundle};
use ozmux_extension_host::terminal_env;

pub struct OzmuxTerminalUiPlugin;

impl Plugin for OzmuxTerminalUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            finish_terminal_setup.in_set(OzmuxSystems::SetupActivity),
        )
        .add_systems(
            PostUpdate,
            resize_terminals_to_node
                .after(UiSystems::Layout)
                .before(UiSystems::PostLayout)
                .before(TerminalMaterialSystems::UpdateMaterial),
        );
    }
}

/// Spawns a `TerminalBundle` and attaches `TerminalRenderBundle` for each
/// freshly-spawned Terminal Activity host. Runs every Update tick but only
/// targets entities that lack `TerminalHandle` and `TerminalSpawnFailed`,
/// so the per-entity work happens exactly once.
///
/// When extensions were launched (the `ExtensionRegistry` resource), the
/// spawned terminal's env is seeded via `terminal_env` with every launched
/// extension's bin dir so any `@<cmd>` shim resolves and can reach the control
/// bridge. The bridge keys on `OZMUX_PANE_ID` being the multiplexer Pane
/// `Entity`, so the host's owning Pane / Session are resolved by walking
/// `ChildOf` from the host's `HostActivityEntity`: activity → Pane → Session.
/// If the chain cannot be resolved (or no extension launched) the env is
/// empty — the terminal still works, just without `@<cmd>` support.
fn finish_terminal_setup(
    mut commands: Commands,
    mut materials: ResMut<Assets<TerminalUiMaterial>>,
    hosts: Query<
        (Entity, &HostActivityEntity),
        (
            With<TerminalActivityMarker>,
            Without<TerminalHandle>,
            Without<TerminalSpawnFailed>,
        ),
    >,
    child_of: Query<&ChildOf>,
    registry: Option<Res<ExtensionRegistry>>,
) {
    for (host, host_activity) in hosts.iter() {
        let env = match registry.as_ref() {
            Some(registry) => match resolve_pane_session(host_activity.0, &child_of) {
                Some((pane, session)) => {
                    let exts: Vec<_> = registry.extensions.values().collect();
                    terminal_env(&exts, pane, session)
                }
                None => Vec::new(),
            },
            None => Vec::new(),
        };
        let opts = SpawnOptions {
            cols: 80,
            rows: 24,
            shell: std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into()),
            cwd: std::env::var_os("HOME").map(std::path::PathBuf::from),
            env,
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

/// Resolves a multiplexer Activity entity to its `(pane, session)` pair by
/// walking `ChildOf` up the multiplexer hierarchy (activity → Pane →
/// Session). Returns `None` when either link is missing — mirrors
/// `MultiplexerCommands::pane_of_activity` + `session_of_pane` without
/// borrowing the full mutation SystemParam.
fn resolve_pane_session(activity: Entity, child_of: &Query<&ChildOf>) -> Option<(Entity, Entity)> {
    let pane = child_of.get(activity).ok()?.parent();
    let session = child_of.get(pane).ok()?.parent();
    Some((pane, session))
}

/// Computes grid dimensions for a host node, reserving `max_overflow_phys`
/// on the right so the WGSL right-strip handler has room to paint the
/// rightmost cell's bbox overflow without clipping. Height is not
/// reserved — line_height already accommodates descender headroom.
///
/// Always returns `(cols, rows)` ≥ `(1, 1)`; degenerate inputs collapse
/// to a 1x1 grid rather than producing zero-sized buffers.
fn compute_grid_dims(
    node_phys_w: f32,
    node_phys_h: f32,
    cell_w_phys: f32,
    cell_h_phys: f32,
    max_overflow_phys: f32,
) -> (u16, u16) {
    let usable_w = (node_phys_w - max_overflow_phys).max(0.0);
    let cols = ((usable_w / cell_w_phys).floor() as u16).max(1);
    let rows = ((node_phys_h / cell_h_phys).floor() as u16).max(1);
    (cols, rows)
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
fn resize_terminals_to_node(
    mut terminals: Query<
        (
            &ComputedNode,
            &mut TerminalHandle,
            &mut PtyHandle,
            &mut Coalescer,
            &mut TerminalGrid,
        ),
        Changed<TerminalHandle>,
    >,
    metrics: Res<TerminalCellMetricsResource>,
) {
    // NOTE: Cell pitch is font-derived physical px; DPR is already baked into
    //       the metrics by `update_terminal_material`'s Resource write-back.
    //       On the first frame after startup (or after a DPR change), the
    //       Resource holds previous-frame values — accepted Tier 1 trade-off,
    //       see `docs/plans/2026-05-25-bevy-font-render-design.md` Tier 2 #11.
    let cell_w_phys = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h_phys = metrics.metrics.line_height_phys.floor().max(1.0);

    for (computed, mut handle, mut pty, mut coalescer, mut grid) in terminals.iter_mut() {
        let (cols, rows) = compute_grid_dims(
            computed.size.x.max(0.0),
            computed.size.y.max(0.0),
            cell_w_phys,
            cell_h_phys,
            metrics.metrics.max_overflow_phys,
        );

        let (cur_cols, cur_rows, _) = handle.read_geometry();
        if cur_cols == cols && cur_rows == rows {
            continue;
        }
        if let Err(e) = handle.resize(&mut pty, &mut coalescer, cols, rows) {
            tracing::warn!(?e, cols, rows, "TerminalHandle::resize failed");
            continue;
        }
        // NOTE: Load-bearing zero-lag short-circuit. Writing the new geometry
        //       into `TerminalGrid` lets `update_terminal_material` use the
        //       correct `grid_size` uniform in the SAME tick. Without it the
        //       shader lags one FrameSnapshot round-trip behind the pane
        //       resize — visible as a strip of shader-fallback color at the
        //       pane edge during a window drag.
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
    fn compute_grid_dims_reserves_max_overflow_for_cols() {
        // Cell pitch 10, node width 100, overflow 4 → cols = (100−4)/10 = 9.
        // Without the reservation: cols = 100/10 = 10. Asserting cols == 9
        // catches both a sign-flip and a use-wrong-field regression.
        let (cols, rows) = compute_grid_dims(100.0, 50.0, 10.0, 10.0, 4.0);
        assert_eq!(cols, 9, "cols should be (100 − 4) / 10 = 9");
        assert_eq!(rows, 5, "rows should be 50 / 10 = 5 (height not affected)");
    }

    #[test]
    fn compute_grid_dims_floor_to_minimum_one() {
        // Degenerate input: overflow exceeds node width.
        let (cols, _) = compute_grid_dims(3.0, 20.0, 10.0, 10.0, 5.0);
        assert_eq!(cols, 1, "cols must stay >= 1 even when usable width is 0");
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
        let activity = app.world_mut().spawn_empty().id();
        let host = app
            .world_mut()
            .spawn((TerminalActivityMarker, HostActivityEntity(activity)))
            .id();
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
