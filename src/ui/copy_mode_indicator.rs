//! tmux-style copy-mode indicator chip. A `Display::None` chip Node is
//! attached as a child of each Activity host the first frame
//! `TerminalHandle` is observed there; it becomes visible while the
//! host carries `CopyModeState` and shows `[offset/total]` over the
//! pane's top-right corner.

use crate::theme;
use crate::ui::palette;
use bevy::app::{App, Plugin};
use bevy::ecs::component::Component;
use bevy::prelude::*;

/// Marker for the chip Node child of an Activity host. Exactly one
/// per host; created on `Added<TerminalHandle>` and never despawned
/// (visibility toggled via `Node.display`).
#[derive(Component)]
pub(crate) struct CopyModeIndicator;

/// Last `(offset, total)` pair this chip rendered. Compared numerically
/// each frame so `format!` only runs when the pair changed.
#[derive(Component, Default, Debug, PartialEq, Eq)]
pub(crate) struct IndicatorCache {
    pub offset: u32,
    pub total: u32,
}

/// Formats the chip body as `[offset/total]` — tmux compatible.
pub(crate) fn format_indicator(offset: u32, total: u32) -> String {
    format!("[{offset}/{total}]")
}

/// Spawns a `CopyModeIndicator` chip as a child of every Activity host
/// the first frame `TerminalHandle` is observed there. The
/// `Added<TerminalHandle>` filter fires exactly once per host because
/// `ui::terminal::finish_terminal_setup` is the only `TerminalHandle`
/// inserter on Activity hosts.
//
// NOTE: A second reader of `Added<TerminalHandle>` would not violate
// the "exactly one chip per host" property (Added fires per-system),
// but introducing one is a smell — the constraint is documented as
// this comment rather than enforced.
fn attach_indicator_to_activity_host(
    mut commands: Commands,
    hosts: Query<Entity, Added<bevy_terminal::TerminalHandle>>,
) {
    for host in hosts.iter() {
        commands.entity(host).with_children(|parent| {
            parent.spawn((
                CopyModeIndicator,
                IndicatorCache::default(),
                Text::new(""),
                BackgroundColor(palette::COPY_MODE_INDICATOR_BG),
                TextColor(palette::COPY_MODE_INDICATOR_FG),
                Node {
                    position_type: PositionType::Absolute,
                    top: Val::Px(0.0),
                    right: Val::Px(0.0),
                    padding: UiRect::axes(
                        Val::Px(theme::ELEMENT_PADDING_PX),
                        Val::Px(0.0),
                    ),
                    display: Display::None,
                    ..default()
                },
            ));
        });
    }
}

/// Bevy Plugin: wires the copy-mode indicator's attach + refresh systems
/// and the exit observer.
pub struct CopyModeIndicatorPlugin;

impl Plugin for CopyModeIndicatorPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, attach_indicator_to_activity_host);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::app::App;
    use bevy::ecs::entity::Entity;
    use bevy_terminal::{SpawnOptions, TerminalBundle};

    #[test]
    fn format_indicator_matches_tmux_default() {
        assert_eq!(format_indicator(0, 429), "[0/429]");
        assert_eq!(format_indicator(3, 429), "[3/429]");
        assert_eq!(format_indicator(0, 0), "[0/0]");
    }

    fn make_app_with_plugin() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(CopyModeIndicatorPlugin);
        app
    }

    fn spawn_terminal_entity(app: &mut App) -> Entity {
        let opts = SpawnOptions {
            cols: 10,
            rows: 5,
            shell: "/bin/sh".into(),
            cwd: None,
            env: Vec::new(),
        };
        let bundle = TerminalBundle::spawn(opts).expect("spawn /bin/sh");
        app.world_mut().spawn(bundle).id()
    }

    fn find_indicator_child(app: &App, host: Entity) -> Option<Entity> {
        let world = app.world();
        let children = world.get::<Children>(host)?;
        children
            .iter()
            .find(|child| world.get::<CopyModeIndicator>(*child).is_some())
    }

    #[test]
    fn attach_spawns_one_indicator_per_terminal_host() {
        let mut app = make_app_with_plugin();
        let host = spawn_terminal_entity(&mut app);

        // One tick: attach system observes Added<TerminalHandle> and
        // queues the chip spawn; the deferred commands flush at the
        // end-of-schedule sync point.
        app.update();

        let chip = find_indicator_child(&app, host).expect("chip must exist");
        let chip_node = app
            .world()
            .get::<Node>(chip)
            .expect("chip must have a Node");
        assert_eq!(
            chip_node.display,
            Display::None,
            "chip starts hidden"
        );
        let cache = app
            .world()
            .get::<IndicatorCache>(chip)
            .expect("chip must carry IndicatorCache");
        assert_eq!(*cache, IndicatorCache::default());
    }

    #[test]
    fn attach_does_not_respawn_across_many_ticks() {
        let mut app = make_app_with_plugin();
        let host = spawn_terminal_entity(&mut app);

        for _ in 0..10 {
            app.update();
        }

        let world = app.world();
        let children = world
            .get::<Children>(host)
            .expect("host must have children");
        let indicator_count = children
            .iter()
            .filter(|c| world.get::<CopyModeIndicator>(*c).is_some())
            .count();
        assert_eq!(indicator_count, 1, "exactly one chip after 10 ticks");
    }
}
