//! Window-fill resize system for the Ozma terminal.

use crate::spawn::{OzmaTerminal, cells_for};
use bevy::ecs::lifecycle::Add;
use bevy::ecs::schedule::common_conditions::any_with_component;
use bevy::prelude::*;
use bevy::window::{PrimaryWindow, WindowResized};
use ozma_tty_engine::{Coalescer, PtyHandle, TerminalHandle};
use ozma_tty_renderer::TerminalCellMetricsResource;

pub(crate) struct LayoutPlugin;

impl Plugin for LayoutPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<OzmaLastSize>()
            .add_message::<WindowResized>()
            .add_observer(reset_last_size)
            .add_systems(
                Update,
                resize_to_window
                    .run_if(any_with_component::<OzmaTerminal>)
                    .run_if(resource_exists::<TerminalCellMetricsResource>)
                    .run_if(
                        resource_exists_and_changed::<OzmaLastSize>
                            .or(resource_exists_and_changed::<TerminalCellMetricsResource>)
                            .or(on_message::<WindowResized>),
                    ),
            );
    }
}

#[derive(Resource, Default)]
struct OzmaLastSize(Option<(u16, u16)>);

fn reset_last_size(_trigger: On<Add, OzmaTerminal>, mut last_size: ResMut<OzmaLastSize>) {
    last_size.0 = None;
}

fn resize_to_window(
    mut commands: Commands,
    mut last_size: ResMut<OzmaLastSize>,
    mut terminal: Query<
        (Entity, &mut TerminalHandle, &mut PtyHandle, &mut Coalescer),
        With<OzmaTerminal>,
    >,
    metrics: Res<TerminalCellMetricsResource>,
    window: Query<&Window, With<PrimaryWindow>>,
) {
    let Ok(window) = window.single() else {
        return;
    };
    let Ok((entity, mut handle, mut pty, mut coalescer)) = terminal.single_mut() else {
        return;
    };

    let cell_w = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h = metrics.metrics.line_height_phys.floor().max(1.0);
    let (cols, rows) = cells_for(
        window.resolution.physical_width(),
        window.resolution.physical_height(),
        cell_w,
        cell_h,
    );

    if last_size.0 == Some((cols, rows)) {
        return;
    }

    match handle.resize(&mut pty, &mut coalescer, cols, rows) {
        Ok(()) => {
            last_size.0 = Some((cols, rows));
            handle.emit_pending(&mut commands, entity);
        }
        Err(e) => tracing::warn!(?e, cols, rows, "failed to resize ozma terminal"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_size_starts_none() {
        assert!(OzmaLastSize::default().0.is_none());
    }

    #[test]
    fn ozma_terminal_spawn_resets_last_size() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<WindowResized>();
        app.init_resource::<OzmaLastSize>();
        app.add_observer(reset_last_size);

        app.world_mut().resource_mut::<OzmaLastSize>().0 = Some((80, 24));
        app.world_mut().spawn(OzmaTerminal);
        app.update();

        assert!(
            app.world().resource::<OzmaLastSize>().0.is_none(),
            "OzmaLastSize should reset to None when OzmaTerminal spawns",
        );
    }
}
