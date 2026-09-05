//! Window-fill resize system for the Orzma terminal.

use crate::surface::OrzmaTerminal;
use crate::surface::geometry::cells_for;
use bevy::ecs::lifecycle::Add;
use bevy::ecs::schedule::common_conditions::any_with_component;
use bevy::prelude::*;
use bevy::window::{PrimaryWindow, WindowResized};
use bevy_orzma_tty::prelude::OrzmaTtyHandle;
use orzma_tty::CellPixels;
use orzma_tty_renderer::TerminalCellMetricsResource;

/// Registers the window-fill resize system.
pub(super) struct LayoutPlugin;

impl Plugin for LayoutPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<OrzmaLastSize>()
            .add_message::<WindowResized>()
            .add_observer(reset_last_size)
            .add_systems(
                Update,
                resize_to_window
                    .run_if(any_with_component::<OrzmaTerminal>)
                    .run_if(resource_exists::<TerminalCellMetricsResource>)
                    .run_if(
                        resource_exists_and_changed::<OrzmaLastSize>
                            .or_else(resource_exists_and_changed::<TerminalCellMetricsResource>)
                            .or_else(on_message::<WindowResized>),
                    ),
            );
    }
}

#[derive(Resource, Default)]
struct OrzmaLastSize(Option<(u16, u16)>);

fn reset_last_size(_trigger: On<Add, OrzmaTerminal>, mut last_size: ResMut<OrzmaLastSize>) {
    last_size.0 = None;
}

fn resize_to_window(
    mut last_size: ResMut<OrzmaLastSize>,
    mut terminal: Query<&mut OrzmaTtyHandle, With<OrzmaTerminal>>,
    metrics: Res<TerminalCellMetricsResource>,
    window: Query<&Window, With<PrimaryWindow>>,
) {
    let Ok(window) = window.single() else {
        return;
    };
    let Ok(mut handle) = terminal.single_mut() else {
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

    match handle.resize(cols, rows, CellPixels::default()) {
        Ok(()) => last_size.0 = Some((cols, rows)),
        Err(e) => tracing::warn!(?e, cols, rows, "failed to resize orzma terminal"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_size_starts_none() {
        assert!(OrzmaLastSize::default().0.is_none());
    }

    #[test]
    fn spawn_of_orzma_terminal_resets_last_size() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<WindowResized>();
        app.init_resource::<OrzmaLastSize>();
        app.add_observer(reset_last_size);

        app.world_mut().resource_mut::<OrzmaLastSize>().0 = Some((80, 24));
        app.world_mut().spawn(OrzmaTerminal);
        app.update();

        assert!(
            app.world().resource::<OrzmaLastSize>().0.is_none(),
            "OrzmaLastSize should reset to None when OrzmaTerminal spawns",
        );
    }
}
