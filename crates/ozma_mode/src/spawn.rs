//! Terminal spawn and despawn for Ozma mode.
use crate::OzmaModeConfig;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use ozma_tty_engine::{SpawnOptions, TerminalBundle};
use ozma_tty_renderer::TerminalCellMetricsResource;
use ozma_tty_renderer::material::TerminalUiMaterial;
use ozma_tty_renderer::prelude::TerminalRenderBundle;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// Marker component identifying the single Ozma terminal entity.
#[derive(Component)]
pub(crate) struct OzmaTerminal;

pub(crate) struct SpawnPlugin;

impl Plugin for SpawnPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(crate::AppMode::Ozma), spawn_terminal)
            .add_systems(OnExit(crate::AppMode::Ozma), despawn_terminal);
    }
}

pub(crate) fn cells_for(w_px: u32, h_px: u32, cell_w: f32, cell_h: f32) -> (u16, u16) {
    let cols = ((w_px as f32 / cell_w).floor() as u16).max(1);
    let rows = ((h_px as f32 / cell_h).floor() as u16).max(1);
    (cols, rows)
}

fn spawn_terminal(
    mut commands: Commands,
    mut materials: ResMut<Assets<TerminalUiMaterial>>,
    mut exit: MessageWriter<AppExit>,
    config: Res<OzmaModeConfig>,
    metrics: Option<Res<TerminalCellMetricsResource>>,
    window_q: Query<&Window, With<PrimaryWindow>>,
) {
    let (cols, rows) = metrics
        .as_ref()
        .zip(window_q.single().ok())
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

fn despawn_terminal(
    mut commands: Commands,
    terminal_q: Query<Entity, With<OzmaTerminal>>,
) {
    for entity in terminal_q.iter() {
        commands.entity(entity).despawn();
    }
}

fn resolve_shell(config: Option<&str>, env_shell: Option<&str>) -> String {
    config
        .filter(|s| !s.is_empty())
        .or_else(|| env_shell.filter(|s| !s.is_empty()))
        .unwrap_or("/bin/sh")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(cells_for(0, 0, 8.0, 16.0), (1, 1));
        assert_eq!(cells_for(807, 607, 8.0, 16.0), (100, 37));
    }
}
