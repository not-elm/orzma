//! Window geometry: computes the whole-window cell size and cell pixel
//! pitch from the primary window and the font metrics, records them in
//! `PaneGeometry`, and sends `OrzmuxCommand::Resize`.

use crate::surface::geometry::{cell_pitch_phys, cells_for};
use bevy::ecs::schedule::common_conditions::on_message;
use bevy::prelude::*;
use bevy::window::{PrimaryWindow, WindowResized};
use bevy_orzmux::prelude::{OrzmuxConnection, PaneGeometry};
use orzma_tty::CellPixels;
use orzma_tty_renderer::TerminalCellMetricsResource;
use orzmux::prelude::OrzmuxCommand;

/// Registers the window-geometry sender.
pub(super) struct LayoutPlugin;

impl Plugin for LayoutPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LastGeometry>()
            .add_message::<WindowResized>()
            .add_systems(
                Update,
                send_window_geometry
                    .run_if(resource_exists::<TerminalCellMetricsResource>)
                    .run_if(
                        not(resource_exists::<PaneGeometry>)
                            .or_else(resource_exists_and_changed::<TerminalCellMetricsResource>)
                            .or_else(on_message::<WindowResized>),
                    ),
            );
    }
}

/// The `(cols, rows, cell_px)` last sent, so a resize that changes
/// nothing sends nothing.
#[derive(Resource, Default)]
struct LastGeometry(Option<(u16, u16, CellPixels)>);

#[expect(
    clippy::cast_possible_truncation,
    reason = "cell_w/cell_h are floored and clamped to at least 1.0 by cells_for's callers, so the u16 cast never truncates a meaningful value"
)]
fn send_window_geometry(
    mut commands: Commands,
    mut last: ResMut<LastGeometry>,
    mut geometry: Option<ResMut<PaneGeometry>>,
    connection: Res<OrzmuxConnection>,
    metrics: Res<TerminalCellMetricsResource>,
    window: Query<&Window, With<PrimaryWindow>>,
) {
    let Ok(window) = window.single() else {
        return;
    };
    let (cell_w, cell_h) = cell_pitch_phys(&metrics.metrics);
    let (cols, rows) = cells_for(
        window.resolution.physical_width(),
        window.resolution.physical_height(),
        cell_w,
        cell_h,
    );
    let cell_px = CellPixels {
        width: cell_w as u16,
        height: cell_h as u16,
    };
    let wanted = PaneGeometry {
        cell_px,
        scale_factor: window.scale_factor(),
    };
    match geometry.as_mut() {
        Some(geometry) => {
            geometry.set_if_neq(wanted);
        }
        None => commands.insert_resource(wanted),
    }
    if last.0 == Some((cols, rows, cell_px)) {
        return;
    }
    last.0 = Some((cols, rows, cell_px));
    connection.0.send(OrzmuxCommand::Resize {
        cols,
        rows,
        cell_px,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::window::WindowResolution;
    use orzma_tty_renderer::CellMetrics;
    use orzmux::prelude::OrzmuxClient;

    fn metrics(advance: f32, line_height: f32) -> TerminalCellMetricsResource {
        TerminalCellMetricsResource {
            metrics: CellMetrics {
                advance_phys: advance,
                line_height_phys: line_height,
                ascent_phys: 0.0,
                descent_phys: 0.0,
                underline_position_phys: 0.0,
                underline_thickness_phys: 0.0,
                max_overflow_phys: 0.0,
            },
            phys_font_size: 12,
        }
    }

    /// Asserts that an unchanged geometry sends no second `Resize` and a
    /// changed window size does, updating `PaneGeometry` alongside.
    ///
    /// Case: two frames pass with the window unchanged, then the user
    /// widens the window.
    #[test]
    fn geometry_is_sent_only_when_it_changes() {
        let (client, _events, commands) = OrzmuxClient::detached();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(LayoutPlugin)
            .insert_resource(OrzmuxConnection(client))
            .insert_resource(metrics(8.0, 16.0));
        let window = app
            .world_mut()
            .spawn((
                Window {
                    resolution: WindowResolution::new(800, 600),
                    ..default()
                },
                PrimaryWindow,
            ))
            .id();
        app.update();
        app.update();
        let sent: Vec<OrzmuxCommand> = commands.try_iter().map(|(_, c)| c).collect();
        assert!(matches!(
            sent.as_slice(),
            [OrzmuxCommand::Resize {
                cols: 100,
                rows: 37,
                ..
            }]
        ));
        assert_eq!(
            app.world().resource::<PaneGeometry>().cell_px,
            CellPixels {
                width: 8,
                height: 16
            }
        );

        app.world_mut()
            .get_mut::<Window>(window)
            .unwrap()
            .resolution = WindowResolution::new(1600, 600);
        app.world_mut().write_message(WindowResized {
            window,
            width: 1600.0,
            height: 600.0,
        });
        app.update();
        let sent: Vec<OrzmuxCommand> = commands.try_iter().map(|(_, c)| c).collect();
        assert!(matches!(
            sent.as_slice(),
            [OrzmuxCommand::Resize {
                cols: 200,
                rows: 37,
                ..
            }]
        ));
    }
}
