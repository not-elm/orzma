//! AppMode state enum and the Ozma single-terminal lifecycle plugin.

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use ozma_terminal::{OzmaTerminal, OzmaTerminalConfig, cells_for, resolve_shell};
use ozma_tty_engine::{SpawnOptions, TerminalBundle};
use ozma_tty_renderer::TerminalCellMetricsResource;
use ozma_tty_renderer::material::TerminalUiMaterial;
use ozma_tty_renderer::prelude::TerminalRenderBundle;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// Application mode. `Ozma` is the default (single PTY, no tmux).
/// `Ozmux` activates the tmux multiplexer backend.
#[derive(States, Debug, Clone, PartialEq, Eq, Hash, Default)]
pub(crate) enum AppMode {
    /// Single PTY terminal, Alacritty VT emulation, no tmux.
    #[default]
    Ozma,
    /// tmux backend, multiplexer layout.
    Ozmux,
}

/// Bevy plugin that registers the `AppMode::Ozma` spawn/despawn lifecycle.
///
/// Spawns one `OzmaTerminal` entity on `OnEnter(AppMode::Ozma)` and
/// despawns it on `OnExit(AppMode::Ozma)`. Requires `AppMode` to be
/// inserted via `App::insert_state` before this plugin runs, and
/// `OzmaTerminalPlugin` must be added first (it inserts `OzmaTerminalConfig`
/// that `spawn_terminal` reads).
pub(crate) struct OzmaModePlugin;

impl Plugin for OzmaModePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppMode::Ozma), spawn_terminal)
            .add_systems(OnExit(AppMode::Ozma), despawn_terminal);
    }
}

fn spawn_terminal(
    mut commands: Commands,
    mut materials: ResMut<Assets<TerminalUiMaterial>>,
    mut exit: MessageWriter<AppExit>,
    config: Res<OzmaTerminalConfig>,
    metrics: Option<Res<TerminalCellMetricsResource>>,
    window: Query<&Window, With<PrimaryWindow>>,
) {
    let (cols, rows) = metrics
        .as_ref()
        .zip(window.single().ok())
        .map(|(m, w)| {
            let cell_w = m.metrics.advance_phys.floor().max(1.0);
            let cell_h = m.metrics.line_height_phys.floor().max(1.0);
            cells_for(
                w.resolution.physical_width(),
                w.resolution.physical_height(),
                cell_w,
                cell_h,
            )
        })
        .unwrap_or((80, 24));

    let shell = resolve_shell(
        config.shell.as_deref(),
        std::env::var("SHELL").ok().as_deref(),
    );

    let opts = SpawnOptions {
        cols,
        rows,
        shell,
        cwd: None,
        env: Vec::new(),
        osc_webview_gate: Arc::new(AtomicBool::new(false)),
    };

    match TerminalBundle::spawn(opts) {
        Ok(bundle) => {
            let material = materials.add(TerminalUiMaterial::default());
            commands.spawn((
                bundle,
                TerminalRenderBundle::new(material),
                OzmaTerminal,
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    top: Val::Px(0.0),
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
            ));
        }
        Err(e) => {
            tracing::error!(?e, "failed to spawn ozma terminal");
            exit.write(AppExit::Success);
        }
    }
}

fn despawn_terminal(mut commands: Commands, terminals: Query<Entity, With<OzmaTerminal>>) {
    for entity in terminals.iter() {
        commands.entity(entity).despawn();
    }
}
