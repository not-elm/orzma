//! Post-setup for Terminal Surface entities. Once the rebuild has decorated a
//! Surface entity with a `Node` + `TerminalSurfaceMarker`, this system spawns
//! a `TerminalBundle` (PTY + VT bridge) and attaches a `TerminalRenderBundle`
//! (renderer-side grid + MaterialNode) directly onto the Surface entity
//! exactly once. Failures mark the entity with `TerminalSpawnFailed` so the
//! system does not retry on subsequent frames.

#[cfg(not(feature = "thin-client"))]
use crate::extension_manager::ExtensionRegistry;
use crate::system_set::OzmuxSystems;
#[cfg(not(feature = "thin-client"))]
use crate::ui::TerminalSpawnFailed;
use crate::ui::TerminalSurfaceMarker;
#[cfg(not(feature = "thin-client"))]
use crate::ui::chrome::PaneChrome;
use bevy::prelude::*;
#[cfg(not(feature = "thin-client"))]
use bevy::ui::UiSystems;
#[cfg(feature = "thin-client")]
use bevy_terminal::TerminalCurrentDir;
#[cfg(not(feature = "thin-client"))]
use bevy_terminal::{PtyHandle, SpawnOptions, TerminalBundle, TerminalCurrentDir, TerminalHandle};
#[cfg(not(feature = "thin-client"))]
use bevy_terminal_renderer::TerminalCellMetricsResource;
#[cfg(not(feature = "thin-client"))]
use bevy_terminal_renderer::material::TerminalMaterialSystems;
use bevy_terminal_renderer::material::TerminalUiMaterial;
use bevy_terminal_renderer::prelude::{TerminalGrid, TerminalRenderBundle};
#[cfg(not(feature = "thin-client"))]
use ozmux_extension_host::terminal_env;
#[cfg(not(feature = "thin-client"))]
use ozmux_multiplexer::PaneDimensions;
use ozmux_multiplexer::{Cwd, OwningWorkspace, PaneMarker, SurfaceOf};

pub struct OzmuxTerminalUiPlugin;

impl Plugin for OzmuxTerminalUiPlugin {
    fn build(&self, app: &mut App) {
        #[cfg(not(feature = "thin-client"))]
        app.add_systems(
            Update,
            finish_terminal_setup.in_set(OzmuxSystems::SetupSurface),
        )
        .add_systems(
            PostUpdate,
            resize_terminals_from_dimensions
                .after(UiSystems::Layout)
                .before(TerminalMaterialSystems::UpdateMaterial),
        );
        #[cfg(feature = "thin-client")]
        app.add_systems(
            Update,
            attach_render_to_surfaces.in_set(OzmuxSystems::SetupSurface),
        );
        app.add_observer(on_terminal_current_dir);
    }
}

/// Thin-client render setup: attaches a `TerminalRenderBundle` to each terminal
/// surface entity that lacks one. The PTY is managed by the daemon; the GUI
/// only needs the render grid + material to display frames from the wire.
#[cfg(feature = "thin-client")]
fn attach_render_to_surfaces(
    mut commands: Commands,
    mut materials: ResMut<Assets<TerminalUiMaterial>>,
    surfaces: Query<Entity, (With<TerminalSurfaceMarker>, Without<TerminalGrid>)>,
) {
    for surface in &surfaces {
        let material = materials.add(TerminalUiMaterial::default());
        commands
            .entity(surface)
            .insert(TerminalRenderBundle::new(material));
    }
}

/// Spawns a `TerminalBundle` and attaches `TerminalRenderBundle` for each
/// freshly-spawned Terminal Surface host. Runs every Update tick but only
/// targets entities that lack `TerminalHandle` and `TerminalSpawnFailed`,
/// so the per-entity work happens exactly once.
///
/// When extensions were launched (the `ExtensionRegistry` resource), the
/// spawned terminal's env is seeded via `terminal_env` with every launched
/// extension's bin dir so any `@<cmd>` shim resolves and can reach the control
/// bridge. The bridge keys on `OZMUX_PANE_ID` being the multiplexer Pane
/// `Entity`, so the surface's owning Pane / Workspace are resolved via the
/// `SurfaceOf` / `OwningWorkspace` relationships: surface → Pane → Workspace.
/// If the chain cannot be resolved (or no extension launched) the env is
/// empty — the terminal still works, just without `@<cmd>` support.
#[cfg(not(feature = "thin-client"))]
fn finish_terminal_setup(
    mut commands: Commands,
    mut materials: ResMut<Assets<TerminalUiMaterial>>,
    surfaces: Query<
        Entity,
        (
            With<TerminalSurfaceMarker>,
            Without<TerminalHandle>,
            Without<TerminalSpawnFailed>,
        ),
    >,
    owners: Query<&SurfaceOf>,
    pane_workspaces: Query<&OwningWorkspace, With<PaneMarker>>,
    registry: Option<Res<ExtensionRegistry>>,
    cwds: Query<&Cwd>,
) {
    for surface in surfaces.iter() {
        let mut env = match registry.as_ref() {
            Some(registry) => match resolve_pane_workspace(surface, &owners, &pane_workspaces) {
                Some((pane, workspace)) => {
                    let exts: Vec<_> = registry.extensions.values().collect();
                    terminal_env(&exts, pane, workspace)
                }
                None => Vec::new(),
            },
            None => Vec::new(),
        };
        env.push(("TERM_PROGRAM".to_string(), "Apple_Terminal".to_string()));
        let seed = cwds.get(surface).ok().map(|c| c.0.clone());
        let opts = SpawnOptions {
            cols: 80,
            rows: 24,
            shell: std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into()),
            cwd: Some(resolve_spawn_cwd(seed)),
            env,
        };
        let bundle = match TerminalBundle::spawn(opts) {
            Ok(b) => b,
            Err(e) => {
                tracing::error!(?e, ?surface, "TerminalBundle::spawn failed");
                commands.entity(surface).insert(TerminalSpawnFailed);
                continue;
            }
        };
        let material_handle = materials.add(TerminalUiMaterial::default());
        commands
            .entity(surface)
            .insert((bundle, TerminalRenderBundle::new(material_handle)));
    }
}

/// Resolves a surface's seed cwd to a concrete spawn directory: the path when
/// it is an absolute, existing directory, else `$HOME` (else `/`). `is_absolute`
/// is load-bearing — `Path::is_dir` resolves a relative path against ozmux's
/// own process cwd.
#[cfg(not(feature = "thin-client"))]
fn resolve_spawn_cwd(cwd: Option<std::path::PathBuf>) -> std::path::PathBuf {
    cwd.filter(|p| p.is_absolute() && p.is_dir())
        .or_else(|| std::env::var_os("HOME").map(std::path::PathBuf::from))
        .unwrap_or_else(|| std::path::PathBuf::from("/"))
}

/// Resolves a multiplexer Surface entity to its `(pane, workspace)` pair via
/// the ownership relationships (surface `SurfaceOf` → Pane `OwningWorkspace` →
/// Workspace), NOT via the layout `ChildOf` tree. This is load-bearing: a
/// parked (inactive) surface is `ChildOf(workspace)`, so walking `ChildOf`
/// would skip its Pane entirely and resolve the wrong owner. Returns `None`
/// when either link is missing — mirrors
/// `MultiplexerCommands::pane_of_surface` + `workspace_of_pane` without
/// borrowing the full mutation SystemParam.
pub(crate) fn resolve_pane_workspace(
    surface: Entity,
    owners: &Query<&SurfaceOf>,
    pane_workspaces: &Query<&OwningWorkspace, With<PaneMarker>>,
) -> Option<(Entity, Entity)> {
    let pane = owners.get(surface).ok()?.0;
    let workspace = pane_workspaces.get(pane).ok()?.0;
    Some((pane, workspace))
}

/// Resizes each Terminal Surface's PTY / VT grid to its Mux-resolved
/// `PaneDimensions` (cols, rows − chrome_rows), making terminal sizing
/// drift-free. `PaneDimensions` is the single authoritative source for pane
/// cell counts; reading it directly avoids the pixel-flooring drift of the
/// old `ComputedNode`-based path.
///
/// chrome_rows = number of pane rows consumed by the tab bar. The tab bar's
/// actual rendered height is read from its `ComputedNode` (after
/// `UiSystems::Layout`) and converted to rows via `ceil(tab_h / cell_h)`.
///
/// Runs in `PostUpdate` after `UiSystems::Layout` so the tab bar
/// `ComputedNode` is current. Also writes the new `cols`/`rows` directly
/// into `TerminalGrid` so the renderer's `update_terminal_material` (also
/// `PostUpdate`) can rebuild the uniform in the same tick — without this
/// short-circuit the new dimensions would only reach the shader after the
/// next `FrameSnapshot` round-trip through alacritty + observers, adding a
/// visible 1-frame lag at the pane edge during drag.
///
/// Columns are taken directly from `PaneDimensions.cols`.
/// NOTE: The pane node width is `cols × floor(advance_phys)` px; the
/// rightmost cell's glyph bbox overflow paints under the next pane's left
/// edge / the overlay divider. This is acceptable — the old px-based
/// `max_overflow_phys` reservation was only needed for the flexbox-measured
/// path. Revisit in T7 smoke if rightmost-glyph clipping is visible.
#[cfg(not(feature = "thin-client"))]
fn resize_terminals_from_dimensions(
    mut terminals: Query<(
        &SurfaceOf,
        &mut TerminalHandle,
        &mut PtyHandle,
        &mut TerminalGrid,
    )>,
    panes: Query<(&PaneDimensions, &PaneChrome), With<PaneMarker>>,
    tab_bar_nodes: Query<&ComputedNode>,
    metrics: Res<TerminalCellMetricsResource>,
) {
    // NOTE: Cell pitch is font-derived physical px; DPR is already baked into
    //       the metrics by `update_terminal_material`'s Resource write-back.
    //       On the first frame after startup (or after a DPR change), the
    //       Resource holds previous-frame values — accepted Tier 1 trade-off,
    //       see `docs/plans/2026-05-25-bevy-font-render-design.md` Tier 2 #11.
    let cell_h_phys = metrics.metrics.line_height_phys.floor().max(1.0);

    for (surface_of, mut handle, mut pty, mut grid) in terminals.iter_mut() {
        let Ok((dim, chrome)) = panes.get(surface_of.0) else {
            continue;
        };

        let chrome_rows: u16 = tab_bar_nodes
            .get(chrome.tab_bar_entity())
            .map(|tab_node| {
                let tab_h = tab_node.size.y.max(0.0);
                (tab_h / cell_h_phys).ceil() as u16
            })
            .unwrap_or(1);

        let cols = dim.cols.max(1);
        let rows = dim.rows.saturating_sub(chrome_rows).max(1);

        let (cur_cols, cur_rows, _) = handle.read_geometry();
        if cur_cols == cols && cur_rows == rows {
            continue;
        }
        if let Err(e) = handle.resize(&mut pty, cols, rows) {
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

/// Writes a terminal's OSC-7-reported directory onto its `Cwd`. The Surface
/// entity *is* the terminal host, so the event targets it directly.
fn on_terminal_current_dir(
    ev: On<TerminalCurrentDir>,
    mut commands: Commands,
    surfaces: Query<(), With<TerminalSurfaceMarker>>,
) {
    let surface = ev.entity;
    let path = &ev.path;
    if surfaces.get(surface).is_ok() {
        commands.entity(surface).try_insert(Cwd(path.clone()));
    }
}

#[cfg(all(test, not(feature = "thin-client")))]
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
            "entity without TerminalSurfaceMarker must not receive TerminalHandle"
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
        let surface = app.world_mut().spawn(TerminalSurfaceMarker).id();
        app.update();
        // SAFETY: env_guard is still held; restore SHELL state so concurrent
        // tests don't see a dirty env.
        unsafe {
            std::env::remove_var("SHELL");
        }
        assert!(
            app.world().get::<TerminalSpawnFailed>(surface).is_some(),
            "spawn failure must mark the surface with TerminalSpawnFailed"
        );
        assert!(
            app.world().get::<TerminalHandle>(surface).is_none(),
            "spawn failure must not leave a TerminalHandle on the surface"
        );
    }

    #[test]
    fn resolve_spawn_cwd_validates_absolute_dir_else_home() {
        let home = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .unwrap();
        assert_eq!(resolve_spawn_cwd(Some(home.clone())), home);
        assert_eq!(resolve_spawn_cwd(None), home);
        assert_eq!(
            resolve_spawn_cwd(Some(std::path::PathBuf::from("relative/x"))),
            home
        );
        assert_eq!(
            resolve_spawn_cwd(Some(std::path::PathBuf::from("/no/such/dir/xyz"))),
            home
        );
    }

    #[test]
    fn current_dir_event_writes_cwd_on_surface() {
        use bevy::prelude::*;
        use bevy_terminal::TerminalCurrentDir;
        use ozmux_multiplexer::Cwd;

        let mut app = App::new();
        app.add_observer(on_terminal_current_dir);
        let surface = app.world_mut().spawn(TerminalSurfaceMarker).id();
        app.world_mut().trigger(TerminalCurrentDir {
            entity: surface,
            path: std::path::PathBuf::from("/tmp/proj"),
        });
        app.world_mut().flush();
        assert_eq!(
            app.world().get::<Cwd>(surface),
            Some(&Cwd(std::path::PathBuf::from("/tmp/proj"))),
        );
    }
}
