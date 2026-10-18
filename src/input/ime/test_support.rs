//! Fixtures shared by the tests of the IME policy and its target resolution.

use crate::input::focus::KeyboardFocused;
use crate::surface::OrzmaTerminal;
use bevy::app::App;
use bevy::ecs::entity::Entity;
use bevy::ecs::hierarchy::ChildOf;
use bevy::math::{IVec4, Vec2};
use bevy::prelude::{MinimalPlugins, default};
use bevy::state::app::StatesPlugin;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy_cef::prelude::FocusedWebview;
use bevy_orzma_tty_renderer::prelude::{
    Baseline, CellMetrics, TerminalCellMetricsResource, TerminalOverlays, TerminalView, Thickness,
    Underline,
};
use bevy_orzma_webview::Webview;
use orzma_vt::prelude::{Cursor, GridColumn, GridLine, GridPoint, InstanceId};
use orzma_webview_host::prelude::MountId;

/// An app holding `FocusedWebview` and 8×16 px cell metrics, with no
/// window.
pub(super) fn policy_app() -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, StatesPlugin));
    app.init_resource::<FocusedWebview>();
    app.insert_resource(TerminalCellMetricsResource {
        metrics: CellMetrics {
            cell_size: Vec2::new(8.0, 16.0),
            baseline: Baseline::new(12.0),
            underline: Underline {
                position: -2.0,
                thickness: Thickness::new(1.0),
            },
            max_overflow: 0.0,
        },
        phys_font_size: 12,
    });
    app
}

/// Spawns a keyboard-focused terminal whose node's top-left corner sits
/// at (100, 100) physical px, with the cursor at column 5, row 10 of a
/// 80×24 viewport.
pub(super) fn spawn_focused_surface(app: &mut App) -> Entity {
    app.world_mut()
        .spawn((
            OrzmaTerminal,
            KeyboardFocused,
            ComputedNode {
                size: Vec2::new(800.0, 600.0),
                ..ComputedNode::DEFAULT
            },
            UiGlobalTransform::from_xy(500.0, 400.0),
            TerminalView {
                cols: 80,
                rows: 24,
                cursor: Some(Cursor {
                    point: GridPoint {
                        line: GridLine(10),
                        column: GridColumn(5),
                    },
                    ..Cursor::default()
                }),
                ..default()
            },
            TerminalOverlays::default(),
        ))
        .id()
}

/// Gives `surface`'s overlay slot 0 the cell rect `rect`
/// (`row, col, rows, cols`), spawns a webview on that slot as its child,
/// and focuses the webview.
pub(super) fn focus_inline_webview(app: &mut App, surface: Entity, rect: IVec4) {
    app.world_mut()
        .get_mut::<TerminalOverlays>(surface)
        .expect("the surface carries overlays")
        .rects[0] = rect;
    let webview = app
        .world_mut()
        .spawn((
            ChildOf(surface),
            Webview::new("webview".into(), InstanceId(1), MountId::new(1), 0, 10, 40),
        ))
        .id();
    app.world_mut().resource_mut::<FocusedWebview>().0 = Some(webview);
}
