//! Surface host Bevy UI builder. The host entity itself is owned by
//! `SurfaceEntityRegistry` and lives across structural rebuilds; this
//! module's `build_surface_host_children` populates its children (the
//! placeholder Row + name Text) on each rebuild.
//!
//! Phase 3+ replaces these placeholder children with a `MaterialNode<
//! TerminalMaterial>` for the GPU terminal grid.

use crate::ui::StructuralNode;
use crate::ui::palette;
use bevy::color::Color;
use bevy::prelude::*;
use bevy::ui::{AlignItems, FlexDirection, JustifyContent, Val};
use ozmux_multiplexer::SurfaceKind;

/// Background color for the Surface placeholder host, chosen by kind.
fn kind_color(kind: &SurfaceKind) -> Color {
    match kind {
        SurfaceKind::Terminal => palette::SURFACE_TERMINAL,
        SurfaceKind::Browser { .. } => palette::SURFACE_BROWSER,
        SurfaceKind::Extension { .. } => palette::SURFACE_EXTENSION,
    }
}

/// Insert / refresh the `Node` bundle on the stable Surface host entity,
/// then spawn its (structural, replaced each rebuild) placeholder children
/// showing the surface name. The host's `Node` is set with
/// `commands.entity(host).insert(...)` so the existing entity remains;
/// children are spawned fresh.
///
/// For `SurfaceKind::Terminal` and `SurfaceKind::Extension` the
/// placeholder children are skipped because a full-size `MaterialNode`
/// (`TerminalUiMaterial` / `WebviewUiMaterial`) covers the host node
/// entirely. For `SurfaceKind::Browser` the host is a `FlexDirection::Column`
/// so the browser renderer can stack a toolbar above a page webview; the
/// placeholder children are also skipped (the browser renderer spawns the
/// real toolbar + webview children). The kind-colored `BackgroundColor` still
/// shows briefly between surface creation and renderer readiness.
pub(crate) fn build_surface_host_children(
    commands: &mut Commands,
    host: Entity,
    kind: &SurfaceKind,
    name: &Name,
) {
    let is_browser = matches!(kind, SurfaceKind::Browser { .. });
    commands.entity(host).insert((
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

    if matches!(
        kind,
        SurfaceKind::Terminal | SurfaceKind::Extension { .. } | SurfaceKind::Browser { .. }
    ) {
        return;
    }

    let row = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                ..default()
            },
            StructuralNode,
            ChildOf(host),
        ))
        .id();

    commands.spawn((
        Text::new(name.as_str().to_string()),
        TextColor(palette::FOREGROUND),
        StructuralNode,
        ChildOf(row),
    ));
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
    fn browser_host_is_column_and_skips_placeholder() {
        let mut world = World::new();
        let host = world.spawn_empty().id();

        let mut queue = CommandQueue::default();
        {
            let mut commands = Commands::new(&mut queue, &world);
            build_surface_host_children(
                &mut commands,
                host,
                &SurfaceKind::Browser {
                    initial_url: Some("https://example.com".into()),
                    profile: BrowserProfile::default(),
                },
                &Name::new("browser"),
            );
        }
        queue.apply(&mut world);

        let node = world.get::<Node>(host).expect("host must have a Node");
        assert_eq!(
            node.flex_direction,
            FlexDirection::Column,
            "browser host must use FlexDirection::Column"
        );
        assert!(
            world.get::<Children>(host).is_none(),
            "browser host must not spawn placeholder children"
        );
    }

    #[test]
    fn terminal_host_skips_placeholder_children() {
        let mut world = World::new();
        let host = world.spawn_empty().id();

        let mut queue = CommandQueue::default();
        {
            let mut commands = Commands::new(&mut queue, &world);
            build_surface_host_children(
                &mut commands,
                host,
                &SurfaceKind::Terminal,
                &Name::new("terminal"),
            );
        }
        queue.apply(&mut world);

        assert!(
            world.get::<Children>(host).is_none(),
            "terminal host must not spawn placeholder children"
        );
    }
}
