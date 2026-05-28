//! `ActivityEntityRegistry` Resource and the `ActivityHostNode` companion
//! marker. Maps `ActivityId` to a stable Bevy `Entity` that survives
//! structural rebuilds of the UI tree. This is the load-bearing piece
//! that lets future `UiMaterial`-backed terminal rendering keep its
//! `Handle<TerminalMaterial>` and prepared GPU resources alive across
//! split/focus changes.

use bevy::prelude::*;
use ozmux_multiplexer::{ActivityId, ActivityKind};
use std::collections::{HashMap, HashSet};

/// `ActivityId → Entity` map. Updated by `rebuild_session_ui_on_data_change`
/// each rebuild via `get_or_spawn` (insert) and `prune` (sweep).
///
/// Also maintains a secondary `Entity → Entity` map for ECS-native callers
/// that resolve activity entities via `MultiplexerCommands` rather than
/// domain `ActivityId`s. Both maps may be active simultaneously during the
/// ECS migration.
#[derive(Resource, Default)]
pub struct ActivityEntityRegistry {
    entities: HashMap<ActivityId, Entity>,
    // NOTE: ECS-native parallel map: activity Bevy Entity → host UI Entity.
    // Populated by insert_for_entity_test during tests and (in production)
    // by the rebuilt UI layer once Task 15+ ports the rebuild pipeline.
    entity_to_host: HashMap<Entity, Entity>,
}

impl ActivityEntityRegistry {
    /// Look up the stable Entity for an Activity, spawning it if absent.
    /// The newly-spawned Entity carries `ActivityHostNode` so the layout
    /// layer only needs to manage the `ChildOf` parent link, not re-insert
    /// the marker every rebuild.
    pub fn get_or_spawn(
        &mut self,
        commands: &mut Commands,
        id: &ActivityId,
        kind: &ActivityKind,
    ) -> Entity {
        if let Some(&existing) = self.entities.get(id) {
            return existing;
        }
        let mut spawn = commands.spawn(crate::ui::ActivityHostNode);
        if matches!(kind, ActivityKind::Terminal) {
            spawn.insert(crate::ui::TerminalActivityMarker);
        }
        let entity = spawn.id();
        self.entities.insert(id.clone(), entity);
        entity
    }

    /// Despawn any Activity Entity whose `ActivityId` is not in `live`,
    /// and remove its map entry. Called at the end of each rebuild so
    /// closed Activities release their GPU/CPU resources.
    pub fn prune(&mut self, commands: &mut Commands, live: &HashSet<ActivityId>) {
        let dead: Vec<ActivityId> = self
            .entities
            .keys()
            .filter(|id| !live.contains(id))
            .cloned()
            .collect();
        for id in dead {
            if let Some(entity) = self.entities.remove(&id) {
                commands.entity(entity).despawn();
            }
        }
    }

    /// Looks up the host entity registered for `id`. Returns `None` when
    /// no host has been spawned yet (e.g. the activity was just created
    /// and the next `rebuild_session_ui_on_data_change` has not run).
    pub fn get(&self, id: &ActivityId) -> Option<Entity> {
        self.entities.get(id).copied()
    }

    /// Looks up the host entity registered for an ECS-native activity
    /// entity. Returns `None` when no host has been registered for the
    /// given activity entity (e.g. the UI rebuild has not yet run).
    pub fn get_by_entity(&self, activity: Entity) -> Option<Entity> {
        self.entity_to_host.get(&activity).copied()
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entities.len()
    }

    /// Iterates `(&ActivityId, Entity)` over every registered host.
    /// Test-only: lets tests snapshot the live host set without querying
    /// the world (which would require a now-fieldless `ActivityHostNode`
    /// to carry the id).
    #[cfg(test)]
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&ActivityId, Entity)> {
        self.entities.iter().map(|(id, &entity)| (id, entity))
    }

    /// Inserts a pre-existing Entity for `id` without going through
    /// `get_or_spawn`. Test-only: lets `src/input.rs` tests register a
    /// fake activity host so forwarding paths can be exercised without
    /// the UI rebuild pipeline.
    #[cfg(test)]
    pub(crate) fn insert_for_test(&mut self, id: ActivityId, entity: Entity) {
        self.entities.insert(id, entity);
    }

    /// Inserts a mapping from ECS-native activity entity to host UI entity.
    /// Test-only: lets tests using `MultiplexerCommands` register activity
    /// hosts without running the full UI rebuild pipeline.
    #[cfg(test)]
    pub(crate) fn insert_for_entity_test(&mut self, activity: Entity, host: Entity) {
        self.entity_to_host.insert(activity, host);
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
    fn get_or_spawn_returns_same_entity_for_same_id() {
        let mut world = World::new();
        world.insert_resource(ActivityEntityRegistry::default());
        let id = ActivityId::new();
        let kind = ActivityKind::Terminal;

        let mut first = None;
        drive(&mut world, |commands, registry| {
            first = Some(registry.get_or_spawn(commands, &id, &kind));
        });

        let mut second = None;
        drive(&mut world, |commands, registry| {
            second = Some(registry.get_or_spawn(commands, &id, &kind));
        });

        assert_eq!(
            first, second,
            "registry must return the same Entity for the same ActivityId"
        );
        assert_eq!(world.resource::<ActivityEntityRegistry>().len(), 1);
    }

    #[test]
    fn prune_removes_unlisted_activities_and_despawns_their_entities() {
        let mut world = World::new();
        world.insert_resource(ActivityEntityRegistry::default());
        let keep = ActivityId::new();
        let drop = ActivityId::new();
        let kind = ActivityKind::Terminal;

        let mut drop_entity = None;
        drive(&mut world, |commands, registry| {
            registry.get_or_spawn(commands, &keep, &kind);
            drop_entity = Some(registry.get_or_spawn(commands, &drop, &kind));
        });

        let mut live: HashSet<ActivityId> = HashSet::new();
        live.insert(keep.clone());
        drive(&mut world, |commands, registry| {
            registry.prune(commands, &live);
        });

        let registry = world.resource::<ActivityEntityRegistry>();
        assert_eq!(registry.len(), 1, "only `keep` should remain");
        assert!(registry.get(&keep).is_some());
        assert!(registry.get(&drop).is_none());
        assert!(
            world.get_entity(drop_entity.unwrap()).is_err(),
            "dropped Activity Entity must be despawned"
        );
    }

    #[test]
    fn get_or_spawn_inserts_terminal_marker_for_terminal_kind() {
        use crate::ui::TerminalActivityMarker;
        let mut world = World::new();
        world.insert_resource(ActivityEntityRegistry::default());
        let id = ActivityId::new();
        let kind = ActivityKind::Terminal;

        let mut entity = None;
        drive(&mut world, |commands, registry| {
            entity = Some(registry.get_or_spawn(commands, &id, &kind));
        });

        assert!(
            world
                .entity(entity.unwrap())
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
        let id = ActivityId::new();
        let kind = ActivityKind::Browser {
            initial_url: None,
            profile: BrowserProfile::default(),
        };

        let mut entity = None;
        drive(&mut world, |commands, registry| {
            entity = Some(registry.get_or_spawn(commands, &id, &kind));
        });

        assert!(
            world
                .entity(entity.unwrap())
                .get::<TerminalActivityMarker>()
                .is_none(),
            "Browser kind must NOT carry TerminalActivityMarker"
        );
    }
}
