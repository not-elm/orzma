//! `ActivityEntityRegistry` Resource and the `ActivityHostNode` companion
//! marker. Maps each Activity entity to a stable host UI entity that
//! survives structural rebuilds. This is the load-bearing piece that
//! lets future `UiMaterial`-backed terminal rendering keep its
//! `Handle<TerminalMaterial>` and prepared GPU resources alive across
//! split/focus changes.

use crate::ui::{ActivityHostNode, HostActivityEntity, TerminalActivityMarker};
use bevy::prelude::*;
use ozmux_multiplexer::{ActivityKind, ActivityMarker};
use std::collections::HashMap;

/// `activity_entity → host_entity` map. Updated lazily by
/// `get_or_spawn` (insert when the activity first appears in a
/// rebuild) and swept by `prune_registry_on_activity_removal`
/// (driven by `RemovedComponents<ActivityMarker>`).
#[derive(Resource, Default)]
pub struct ActivityEntityRegistry {
    hosts: HashMap<Entity, Entity>,
}

impl ActivityEntityRegistry {
    /// Look up the stable host entity for an Activity, spawning it if
    /// absent. The newly-spawned host carries `ActivityHostNode` so the
    /// layout layer only needs to manage the `ChildOf` parent link, not
    /// re-insert the marker every rebuild.
    pub fn get_or_spawn(
        &mut self,
        commands: &mut Commands,
        activity: Entity,
        kind: &ActivityKind,
    ) -> Entity {
        if let Some(&existing) = self.hosts.get(&activity) {
            return existing;
        }
        let mut spawn = commands.spawn((ActivityHostNode, HostActivityEntity(activity)));
        if matches!(kind, ActivityKind::Terminal) {
            spawn.insert(TerminalActivityMarker);
        }
        let host = spawn.id();
        self.hosts.insert(activity, host);
        host
    }

    /// Look up the host entity for an Activity. Returns `None` when
    /// no host has been spawned yet.
    pub fn get(&self, activity: Entity) -> Option<Entity> {
        self.hosts.get(&activity).copied()
    }

    #[cfg(test)]
    pub(crate) fn len_for_test(&self) -> usize {
        self.hosts.len()
    }

    #[cfg(test)]
    pub(crate) fn iter_for_test(&self) -> impl Iterator<Item = (Entity, Entity)> + '_ {
        self.hosts.iter().map(|(&a, &h)| (a, h))
    }

    /// Inserts a mapping from ECS-native activity entity to host UI entity.
    /// Test-only: lets tests using `MultiplexerCommands` register activity
    /// hosts without running the full UI rebuild pipeline.
    #[cfg(test)]
    pub(crate) fn insert_for_test(&mut self, activity: Entity, host: Entity) {
        self.hosts.insert(activity, host);
    }
}

/// System that drops registry entries for Activity entities that have
/// been despawned. Driven by `RemovedComponents<ActivityMarker>`. Runs
/// every frame; no-ops when no Activities were removed.
pub fn prune_registry_on_activity_removal(
    mut registry: ResMut<ActivityEntityRegistry>,
    mut removed: RemovedComponents<ActivityMarker>,
    mut commands: Commands,
) {
    for activity in removed.read() {
        if let Some(host) = registry.hosts.remove(&activity) {
            commands.entity(host).despawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drive(world: &mut World, f: impl FnOnce(&mut Commands, &mut ActivityEntityRegistry)) {
        let mut registry = world.remove_resource::<ActivityEntityRegistry>().unwrap();
        let mut queue = bevy::ecs::world::CommandQueue::default();
        {
            let mut commands = Commands::new(&mut queue, world);
            f(&mut commands, &mut registry);
        }
        world.insert_resource(registry);
        queue.apply(world);
    }

    #[test]
    fn get_or_spawn_returns_same_entity_for_same_activity_entity() {
        let mut world = World::new();
        world.insert_resource(ActivityEntityRegistry::default());
        let activity = world.spawn_empty().id();
        let kind = ActivityKind::Terminal;

        let mut first = None;
        drive(&mut world, |commands, registry| {
            first = Some(registry.get_or_spawn(commands, activity, &kind));
        });

        let mut second = None;
        drive(&mut world, |commands, registry| {
            second = Some(registry.get_or_spawn(commands, activity, &kind));
        });

        assert_eq!(
            first, second,
            "registry must return the same host Entity for the same activity Entity"
        );
        assert_eq!(world.resource::<ActivityEntityRegistry>().len_for_test(), 1);
    }

    #[test]
    fn prune_system_removes_despawned_activity_and_despawns_its_host() {
        use bevy::MinimalPlugins;
        use bevy::app::App;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(ActivityEntityRegistry::default());
        app.add_systems(bevy::app::Update, super::prune_registry_on_activity_removal);

        let keep_activity = app.world_mut().spawn(ActivityMarker).id();
        let drop_activity = app.world_mut().spawn(ActivityMarker).id();
        let kind = ActivityKind::Terminal;

        let mut drop_host = None;
        {
            let mut registry = app
                .world_mut()
                .remove_resource::<ActivityEntityRegistry>()
                .unwrap();
            let mut queue = bevy::ecs::world::CommandQueue::default();
            {
                let mut commands = Commands::new(&mut queue, app.world());
                registry.get_or_spawn(&mut commands, keep_activity, &kind);
                drop_host = Some(registry.get_or_spawn(&mut commands, drop_activity, &kind));
            }
            app.world_mut().insert_resource(registry);
            queue.apply(app.world_mut());
        }

        app.world_mut().despawn(drop_activity);
        app.update();

        let registry = app.world().resource::<ActivityEntityRegistry>();
        assert_eq!(registry.len_for_test(), 1, "only `keep` should remain");
        assert!(registry.get(keep_activity).is_some());
        assert!(registry.get(drop_activity).is_none());
        assert!(
            app.world().get_entity(drop_host.unwrap()).is_err(),
            "dropped host Entity must be despawned"
        );
    }

    #[test]
    fn get_or_spawn_inserts_terminal_marker_for_terminal_kind() {
        use crate::ui::TerminalActivityMarker;
        let mut world = World::new();
        world.insert_resource(ActivityEntityRegistry::default());
        let activity = world.spawn_empty().id();
        let kind = ActivityKind::Terminal;

        let mut host = None;
        drive(&mut world, |commands, registry| {
            host = Some(registry.get_or_spawn(commands, activity, &kind));
        });

        assert!(
            world
                .entity(host.unwrap())
                .get::<TerminalActivityMarker>()
                .is_some(),
            "Terminal kind must carry TerminalActivityMarker"
        );
    }

    #[test]
    fn get_or_spawn_omits_terminal_marker_for_browser_kind() {
        use crate::ui::TerminalActivityMarker;
        use ozmux_multiplexer::BrowserProfile;
        let mut world = World::new();
        world.insert_resource(ActivityEntityRegistry::default());
        let activity = world.spawn_empty().id();
        let kind = ActivityKind::Browser {
            initial_url: None,
            profile: BrowserProfile::default(),
        };

        let mut host = None;
        drive(&mut world, |commands, registry| {
            host = Some(registry.get_or_spawn(commands, activity, &kind));
        });

        assert!(
            world
                .entity(host.unwrap())
                .get::<TerminalActivityMarker>()
                .is_none(),
            "Browser kind must NOT carry TerminalActivityMarker"
        );
    }
}
