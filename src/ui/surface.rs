//! Surface decoration: the multiplexer Surface entity *is* its own render
//! host. `decorate_surface` stamps the `Node` bundle (+ kind-colored
//! background and the kind-marker) onto the Surface entity per its
//! `SurfaceKind`, so the renderers (`ui::terminal`, `extension_render`,
//! `browser_render`) can attach their material / webview directly to it.

use crate::ui::{BrowserSurfaceMarker, ExtensionSurfaceMarker, TerminalSurfaceMarker, palette};
use bevy::color::Color;
use bevy::prelude::*;
use bevy::ui::{AlignItems, FlexDirection, JustifyContent, Val};
use ozmux_multiplexer::SurfaceKind;

/// Background color for the Surface entity, chosen by kind.
fn kind_color(kind: &SurfaceKind) -> Color {
    match kind {
        SurfaceKind::Terminal => palette::SURFACE_TERMINAL,
        SurfaceKind::Browser { .. } => palette::SURFACE_BROWSER,
        SurfaceKind::Extension { .. } => palette::SURFACE_EXTENSION,
    }
}

/// Inserts / refreshes the `Node` bundle, kind-colored `BackgroundColor`, and
/// the kind-marker (`TerminalSurfaceMarker` / `ExtensionSurfaceMarker` /
/// `BrowserSurfaceMarker`) on the Surface entity. A full-size `MaterialNode`
/// (`TerminalUiMaterial` / `WebviewUiMaterial`) attached later by the renderer
/// covers the node entirely; the kind-colored background shows briefly between
/// surface creation and renderer readiness. For `SurfaceKind::Browser` the
/// node is a `FlexDirection::Column` so the browser renderer can stack a
/// toolbar above a page webview.
pub(crate) fn decorate_surface(commands: &mut Commands, surface: Entity, kind: &SurfaceKind) {
    let is_browser = matches!(kind, SurfaceKind::Browser { .. });
    let mut entity = commands.entity(surface);
    entity.insert((
        Node {
            flex_grow: 1.0,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            flex_direction: if is_browser {
                FlexDirection::Column
            } else {
                FlexDirection::Row
            },
            justify_content: if is_browser {
                JustifyContent::FlexStart
            } else {
                JustifyContent::Center
            },
            align_items: if is_browser {
                AlignItems::Stretch
            } else {
                AlignItems::Center
            },
            ..default()
        },
        BackgroundColor(kind_color(kind)),
    ));
    match kind {
        SurfaceKind::Terminal => {
            entity.insert(TerminalSurfaceMarker);
        }
        SurfaceKind::Extension { .. } => {
            entity.insert(ExtensionSurfaceMarker);
        }
        SurfaceKind::Browser { .. } => {
            entity.insert(BrowserSurfaceMarker);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::world::CommandQueue;
    use ozmux_multiplexer::BrowserProfile;

    #[test]
    fn kind_color_terminal_uses_surface_terminal_constant() {
        assert_eq!(
            kind_color(&SurfaceKind::Terminal),
            palette::SURFACE_TERMINAL
        );
    }

    #[test]
    fn kind_color_browser_uses_surface_browser_constant() {
        let kind = SurfaceKind::Browser {
            initial_url: None,
            profile: BrowserProfile::default(),
        };
        assert_eq!(kind_color(&kind), palette::SURFACE_BROWSER);
    }

    #[test]
    fn browser_surface_is_column_and_carries_browser_marker() {
        let mut world = World::new();
        let surface = world.spawn_empty().id();

        let mut queue = CommandQueue::default();
        {
            let mut commands = Commands::new(&mut queue, &world);
            decorate_surface(
                &mut commands,
                surface,
                &SurfaceKind::Browser {
                    initial_url: Some("https://example.com".into()),
                    profile: BrowserProfile::default(),
                },
            );
        }
        queue.apply(&mut world);

        let node = world.get::<Node>(surface).expect("surface must have a Node");
        assert_eq!(
            node.flex_direction,
            FlexDirection::Column,
            "browser surface must use FlexDirection::Column"
        );
        assert!(
            world.get::<BrowserSurfaceMarker>(surface).is_some(),
            "browser surface must carry BrowserSurfaceMarker"
        );
    }

    #[test]
    fn terminal_surface_carries_terminal_marker() {
        let mut world = World::new();
        let surface = world.spawn_empty().id();

        let mut queue = CommandQueue::default();
        {
            let mut commands = Commands::new(&mut queue, &world);
            decorate_surface(&mut commands, surface, &SurfaceKind::Terminal);
        }
        queue.apply(&mut world);

        assert!(
            world.get::<TerminalSurfaceMarker>(surface).is_some(),
            "terminal surface must carry TerminalSurfaceMarker"
        );
        assert!(
            world.get::<Node>(surface).is_some(),
            "terminal surface must have a Node"
        );
    }
}
