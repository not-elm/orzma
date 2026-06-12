//! Surface decoration: the multiplexer Surface entity *is* its own render
//! host. `decorate_surface` stamps the `Node` bundle (+ kind-colored
//! background and the kind-marker) onto the Surface entity per its
//! `SurfaceKind`, so the renderers (`ui::terminal`, `extension_render`) can
//! attach their material / webview directly to it.

use crate::ui::{ExtensionSurfaceMarker, TerminalSurfaceMarker, palette};
use bevy::color::Color;
use bevy::prelude::*;
use bevy::ui::{AlignItems, FlexDirection, JustifyContent, Val};
use ozmux_multiplexer::SurfaceKind;

/// Background color for the Surface entity, chosen by kind.
fn kind_color(kind: &SurfaceKind) -> Color {
    match kind {
        SurfaceKind::Terminal => palette::SURFACE_TERMINAL,
        SurfaceKind::Extension { .. } => palette::SURFACE_EXTENSION,
    }
}

/// Inserts / refreshes the `Node` bundle, kind-colored `BackgroundColor`, and
/// the kind-marker (`TerminalSurfaceMarker` / `ExtensionSurfaceMarker`) on the
/// Surface entity. A full-size `MaterialNode` (`TerminalUiMaterial` /
/// `WebviewUiMaterial`) attached later by the renderer covers the node
/// entirely; the kind-colored background shows briefly between surface
/// creation and renderer readiness.
pub(crate) fn decorate_surface(commands: &mut Commands, surface: Entity, kind: &SurfaceKind) {
    let mut entity = commands.entity(surface);
    entity.insert((
        Node {
            flex_grow: 1.0,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            flex_direction: FlexDirection::Row,
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::world::CommandQueue;

    #[test]
    fn kind_color_terminal_uses_surface_terminal_constant() {
        assert_eq!(
            kind_color(&SurfaceKind::Terminal),
            palette::SURFACE_TERMINAL
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
