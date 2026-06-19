//! Terminal component types and dimension helpers for Ozma mode.

use bevy::prelude::*;
use ozma_tty_engine::{SpawnOptions, TerminalBundle};
use ozma_tty_renderer::material::TerminalUiMaterial;
use ozma_tty_renderer::prelude::TerminalRenderBundle;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// Marker component identifying the single Ozma-mode terminal entity.
///
/// Exactly one entity carries this marker while the terminal is active.
#[derive(Component)]
pub struct OzmaTerminal;

/// Shell override resource.
///
/// `None` means fall back to `$SHELL` at spawn time.
#[derive(Resource)]
pub struct OzmaTerminalConfig {
    /// Optional shell path. When set, overrides `$SHELL` and `/bin/sh`.
    pub shell: Option<String>,
}

/// Options for spawning a standalone Ozma terminal.
#[derive(Default)]
pub struct OzmaSpawnOptions {
    /// Shell override; `None` falls back to `$SHELL` then `/bin/sh`.
    pub shell: Option<String>,
    /// Working directory for the PTY; `None` inherits the process cwd.
    pub cwd: Option<PathBuf>,
    /// Extra environment variables for the PTY.
    pub env: Vec<(String, String)>,
}

/// Self-contained spawn bundle for a standalone Ozma terminal: the engine PTY
/// bundle, the `OzmaTerminal` marker, and a default full-screen `Node`. The
/// GPU render bundle is injected by `on_add_inject_render` on insertion.
#[derive(Bundle)]
pub struct OzmaTerminalBundle {
    terminal: TerminalBundle,
    marker: OzmaTerminal,
    node: Node,
}

impl OzmaTerminalBundle {
    /// Spawns the PTY at a provisional 80x24 (the window-fill resize system
    /// corrects it on the first frame) and returns the bundle. Errors when the
    /// PTY fails to spawn.
    pub fn spawn(opts: OzmaSpawnOptions) -> anyhow::Result<Self> {
        let shell = resolve_shell(
            opts.shell.as_deref(),
            std::env::var("SHELL").ok().as_deref(),
        );
        let terminal = TerminalBundle::spawn(SpawnOptions {
            cols: 80,
            rows: 24,
            shell,
            cwd: opts.cwd,
            env: opts.env,
            osc_webview_gate: Arc::new(AtomicBool::new(false)),
        })?;
        Ok(Self {
            terminal,
            marker: OzmaTerminal,
            node: Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
        })
    }
}

/// Computes terminal dimensions in cells from physical pixel size.
///
/// Returns `(cols, rows)`, each clamped to a minimum of 1.
pub fn cells_for(w_px: u32, h_px: u32, cell_w: f32, cell_h: f32) -> (u16, u16) {
    let cols = ((w_px as f32 / cell_w).floor() as u16).max(1);
    let rows = ((h_px as f32 / cell_h).floor() as u16).max(1);
    (cols, rows)
}

/// Resolves the shell path: config → `$SHELL` → `/bin/sh`.
pub fn resolve_shell(config: Option<&str>, env_shell: Option<&str>) -> String {
    config
        .filter(|s| !s.is_empty())
        .or_else(|| env_shell.filter(|s| !s.is_empty()))
        .unwrap_or("/bin/sh")
        .to_string()
}

/// Bevy observer that injects a `TerminalRenderBundle` whenever `OzmaTerminal`
/// is added to an entity, allocating the GPU material on demand.
pub(crate) fn on_add_inject_render(
    ev: On<Add, OzmaTerminal>,
    mut commands: Commands,
    mut materials: ResMut<Assets<TerminalUiMaterial>>,
) {
    let material = materials.add(TerminalUiMaterial::default());
    commands
        .entity(ev.event_target())
        .insert(TerminalRenderBundle::new(material));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_add_injects_render_bundle() {
        use bevy::asset::AssetPlugin;
        use ozma_tty_renderer::material::TerminalUiMaterial;
        use ozma_tty_renderer::schema::TerminalGrid;

        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()));
        app.init_resource::<Assets<TerminalUiMaterial>>();
        app.add_observer(on_add_inject_render);
        let entity = app.world_mut().spawn(OzmaTerminal).id();
        app.update();
        assert!(
            app.world().entity(entity).contains::<TerminalGrid>(),
            "On<Add, OzmaTerminal> must inject TerminalRenderBundle (TerminalGrid)",
        );
    }

    #[test]
    fn shell_resolution_uses_config() {
        assert_eq!(
            resolve_shell(Some("/bin/fish"), Some("/bin/zsh")),
            "/bin/fish"
        );
    }

    #[test]
    fn shell_resolution_falls_back_to_env() {
        assert_eq!(resolve_shell(None, Some("/bin/zsh")), "/bin/zsh");
    }

    #[test]
    fn shell_resolution_falls_back_to_sh() {
        assert_eq!(resolve_shell(None, None), "/bin/sh");
    }

    #[test]
    fn cells_for_divides_and_floors() {
        assert_eq!(cells_for(800, 600, 8.0, 16.0), (100, 37));
        assert_eq!(cells_for(1, 1, 8.0, 16.0), (1, 1));
        assert_eq!(cells_for(0, 0, 8.0, 16.0), (1, 1));
        assert_eq!(cells_for(807, 607, 8.0, 16.0), (100, 37));
    }
}
