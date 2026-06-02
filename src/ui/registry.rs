//! `SurfaceEntityRegistry` Resource and the `SurfaceHostNode` companion
//! marker. Maps each Surface entity to a stable host UI entity that
//! survives structural rebuilds. This is the load-bearing piece that
//! lets future `UiMaterial`-backed terminal rendering keep its
//! `Handle<TerminalMaterial>` and prepared GPU resources alive across
//! split/focus changes.

use crate::ui::{
    SurfaceHostNode, BrowserSurfaceMarker, ExtensionSurfaceMarker, HostSurfaceEntity,
    TerminalSurfaceMarker,
};
use bevy::prelude::*;
use ozmux_multiplexer::{SurfaceKind, SurfaceMarker};
use std::collections::HashMap;

/// `surface_entity → host_entity` map. Updated lazily by
/// `get_or_spawn` (insert when the surface first appears in a
/// rebuild) and swept by `prune_registry_on_surface_removal`
/// (driven by `RemovedComponents<SurfaceMarker>`).
#[derive(Resource, Default)]
pub struct SurfaceEntityRegistry {
    hosts: HashMap<Entity, Entity>,
}

impl SurfaceEntityRegistry {
    /// Look up the stable host entity for a Surface, spawning it if
    /// absent. The newly-spawned host carries `SurfaceHostNode` so the
    /// layout layer only needs to manage the `ChildOf` parent link, not
    /// re-insert the marker every rebuild.
    pub fn get_or_spawn(
        &mut self,
        commands: &mut Commands,
        surface: Entity,
        kind: &SurfaceKind,
    ) -> Entity {
        if let Some(&existing) = self.hosts.get(&surface) {
            return existing;
        }
        let mut spawn = commands.spawn((SurfaceHostNode, HostSurfaceEntity(surface)));
        if matches!(kind, SurfaceKind::Terminal) {
            spawn.insert(TerminalSurfaceMarker);
        }
        if matches!(kind, SurfaceKind::Extension { .. }) {
            spawn.insert(ExtensionSurfaceMarker);
        }
        if matches!(kind, SurfaceKind::Browser { .. }) {
            spawn.insert(BrowserSurfaceMarker);
        }
        let host = spawn.id();
        self.hosts.insert(surface, host);
        host
    }

    /// Look up the host entity for a Surface. Returns `None` when
    /// no host has been spawned yet.
    pub fn get(&self, surface: Entity) -> Option<Entity> {
        self.hosts.get(&surface).copied()
    }

    #[cfg(test)]
    pub(crate) fn len_for_test(&self) -> usize {
        self.hosts.len()
    }

    #[cfg(test)]
    pub(crate) fn iter_for_test(&self) -> impl Iterator<Item = (Entity, Entity)> + '_ {
        self.hosts.iter().map(|(&a, &h)| (a, h))
    }

    /// Inserts a mapping from ECS-native surface entity to host UI entity.
    /// Test-only: lets tests using `MultiplexerCommands` register surface
    /// hosts without running the full UI rebuild pipeline.
    #[cfg(test)]
    pub(crate) fn insert_for_test(&mut self, surface: Entity, host: Entity) {
        self.hosts.insert(surface, host);
    }
}

/// System that drops registry entries for Surface entities that have
/// been despawned. Driven by `RemovedComponents<SurfaceMarker>`. Runs
/// every frame; no-ops when no Surfaces were removed.
pub fn prune_registry_on_surface_removal(
    mut registry: ResMut<SurfaceEntityRegistry>,
    mut removed: RemovedComponents<SurfaceMarker>,
    mut commands: Commands,
) {
    for surface in removed.read() {
        if let Some(host) = registry.hosts.remove(&surface) {
            commands.entity(host).despawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drive(world: &mut World, f: impl FnOnce(&mut Commands, &mut SurfaceEntityRegistry)) {
        let mut registry = world.remove_resource::<SurfaceEntityRegistry>().unwrap();
        let mut queue = bevy::ecs::world::CommandQueue::default();
        {
            let mut commands = Commands::new(&mut queue, world);
            f(&mut commands, &mut registry);
        }
        world.insert_resource(registry);
        queue.apply(world);
    }

    #[test]
    fn get_or_spawn_returns_same_entity_for_same_surface_entity() {
        let mut world = World::new();
        world.insert_resource(SurfaceEntityRegistry::default());
        let surface = world.spawn_empty().id();
        let kind = SurfaceKind::Terminal;

        let mut first = None;
        drive(&mut world, |commands, registry| {
            first = Some(registry.get_or_spawn(commands, surface, &kind));
        });

        let mut second = None;
        drive(&mut world, |commands, registry| {
            second = Some(registry.get_or_spawn(commands, surface, &kind));
        });

        assert_eq!(
            first, second,
            "registry must return the same host Entity for the same surface Entity"
        );
        assert_eq!(world.resource::<SurfaceEntityRegistry>().len_for_test(), 1);
    }

    #[test]
    fn prune_system_removes_despawned_surface_and_despawns_its_host() {
        use bevy::MinimalPlugins;
        use bevy::app::App;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(SurfaceEntityRegistry::default());
        app.add_systems(bevy::app::Update, super::prune_registry_on_surface_removal);

        let keep_surface = app.world_mut().spawn(SurfaceMarker).id();
        let drop_surface = app.world_mut().spawn(SurfaceMarker).id();
        let kind = SurfaceKind::Terminal;

        let drop_host;
        {
            let mut registry = app
                .world_mut()
                .remove_resource::<SurfaceEntityRegistry>()
                .unwrap();
            let mut queue = bevy::ecs::world::CommandQueue::default();
            {
                let mut commands = Commands::new(&mut queue, app.world());
                registry.get_or_spawn(&mut commands, keep_surface, &kind);
                drop_host = registry.get_or_spawn(&mut commands, drop_surface, &kind);
            }
            app.world_mut().insert_resource(registry);
            queue.apply(app.world_mut());
        }

        app.world_mut().despawn(drop_surface);
        app.update();

        let registry = app.world().resource::<SurfaceEntityRegistry>();
        assert_eq!(registry.len_for_test(), 1, "only `keep` should remain");
        assert!(registry.get(keep_surface).is_some());
        assert!(registry.get(drop_surface).is_none());
        assert!(
            app.world().get_entity(drop_host).is_err(),
            "dropped host Entity must be despawned"
        );
    }

    #[test]
    fn get_or_spawn_inserts_terminal_marker_for_terminal_kind() {
        use crate::ui::TerminalSurfaceMarker;
        let mut world = World::new();
        world.insert_resource(SurfaceEntityRegistry::default());
        let surface = world.spawn_empty().id();
        let kind = SurfaceKind::Terminal;

        let mut host = None;
        drive(&mut world, |commands, registry| {
            host = Some(registry.get_or_spawn(commands, surface, &kind));
        });

        assert!(
            world
                .entity(host.unwrap())
                .get::<TerminalSurfaceMarker>()
                .is_some(),
            "Terminal kind must carry TerminalSurfaceMarker"
        );
    }

    #[test]
    fn get_or_spawn_inserts_extension_marker_for_extension_kind() {
        use crate::ui::ExtensionSurfaceMarker;
        use std::path::PathBuf;
        let mut world = World::new();
        world.insert_resource(SurfaceEntityRegistry::default());
        let surface = world.spawn_empty().id();
        let kind = SurfaceKind::Extension {
            entry: PathBuf::from("/tmp/memo"),
        };

        let mut host = None;
        drive(&mut world, |commands, registry| {
            host = Some(registry.get_or_spawn(commands, surface, &kind));
        });

        assert!(
            world
                .entity(host.unwrap())
                .get::<ExtensionSurfaceMarker>()
                .is_some(),
            "Extension kind must carry ExtensionSurfaceMarker"
        );
        assert!(
            world
                .entity(host.unwrap())
                .get::<TerminalSurfaceMarker>()
                .is_none(),
            "Extension kind must NOT carry TerminalSurfaceMarker"
        );
    }

    #[test]
    fn get_or_spawn_omits_terminal_marker_for_browser_kind() {
        use crate::ui::TerminalSurfaceMarker;
        use ozmux_multiplexer::BrowserProfile;
        let mut world = World::new();
        world.insert_resource(SurfaceEntityRegistry::default());
        let surface = world.spawn_empty().id();
        let kind = SurfaceKind::Browser {
            initial_url: None,
            profile: BrowserProfile::default(),
        };

        let mut host = None;
        drive(&mut world, |commands, registry| {
            host = Some(registry.get_or_spawn(commands, surface, &kind));
        });

        assert!(
            world
                .entity(host.unwrap())
                .get::<TerminalSurfaceMarker>()
                .is_none(),
            "Browser kind must NOT carry TerminalSurfaceMarker"
        );
    }

    #[test]
    fn get_or_spawn_tags_browser_host_with_browser_marker() {
        use crate::ui::BrowserSurfaceMarker;
        use ozmux_multiplexer::BrowserProfile;
        let mut world = World::new();
        world.insert_resource(SurfaceEntityRegistry::default());
        let surface = world.spawn_empty().id();
        let kind = SurfaceKind::Browser {
            initial_url: Some("https://example.com".into()),
            profile: BrowserProfile::default(),
        };

        let mut host = None;
        drive(&mut world, |commands, registry| {
            host = Some(registry.get_or_spawn(commands, surface, &kind));
        });

        assert!(
            world
                .entity(host.unwrap())
                .get::<BrowserSurfaceMarker>()
                .is_some(),
            "a Browser kind host must carry BrowserSurfaceMarker"
        );
    }
}
