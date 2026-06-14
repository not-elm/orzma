//! Surface decoration: the multiplexer Surface entity *is* its own render
//! host. `decorate_surface` stamps the `Node` bundle, terminal background, and
//! the `TerminalSurfaceMarker` onto the Surface entity, so the terminal
//! renderer (`ui::terminal`) can attach its material directly to it.

use crate::ui::{TerminalSurfaceMarker, palette};
use bevy::prelude::*;
use bevy::ui::{AlignItems, FlexDirection, JustifyContent, Val};

/// Inserts the surface's flex `Node`, terminal background, and the
/// `TerminalSurfaceMarker` that `finish_terminal_setup` queries.
pub(crate) fn decorate_surface(commands: &mut Commands, surface: Entity) {
    commands.entity(surface).insert((
        Node {
            flex_grow: 1.0,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            flex_direction: FlexDirection::Row,
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        BackgroundColor(palette::SURFACE_TERMINAL),
        TerminalSurfaceMarker,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::world::CommandQueue;

    #[test]
    fn terminal_surface_carries_terminal_marker() {
        let mut world = World::new();
        let surface = world.spawn_empty().id();

        let mut queue = CommandQueue::default();
        {
            let mut commands = Commands::new(&mut queue, &world);
            decorate_surface(&mut commands, surface);
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
