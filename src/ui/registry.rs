//! `ActivityEntityRegistry` Resource and the `ActivityHostNode` companion
//! marker. Maps `ActivityId` to a stable Bevy `Entity` that survives
//! structural rebuilds of the UI tree. This is the load-bearing piece
//! that lets future `UiMaterial`-backed terminal rendering keep its
//! `Handle<TerminalMaterial>` and prepared GPU resources alive across
//! split/focus changes.

use bevy::prelude::*;
use ozmux_multiplexer::{ActivityId, ActivityKind};
use std::collections::{HashMap, HashSet};

/// `ActivityId → Entity` map. Updated by `rebuild_structure_on_change`
/// each rebuild via `get_or_spawn` (insert) and `prune` (sweep).
#[derive(Resource, Default)]
pub(crate) struct ActivityEntityRegistry {
    entities: HashMap<ActivityId, Entity>,
}

impl ActivityEntityRegistry {
    /// Look up the stable Entity for an Activity, spawning it if absent.
    /// The newly-spawned Entity carries `ActivityHostNode` so the layout
    /// layer only needs to manage the `ChildOf` parent link, not re-insert
    /// the marker every rebuild.
    pub(crate) fn get_or_spawn(
        &mut self,
        commands: &mut Commands,
        id: &ActivityId,
        _kind: &ActivityKind,
    ) -> Entity {
        if let Some(&existing) = self.entities.get(id) {
            return existing;
        }
        // TODO: Phase 3+ will branch on `_kind` to spawn kind-specific bundles
        // (e.g., MaterialNode<TerminalMaterial> for ActivityKind::Terminal).
        let entity = commands.spawn(crate::ui::ActivityHostNode(id.clone())).id();
        self.entities.insert(id.clone(), entity);
        entity
    }

    /// Despawn any Activity Entity whose `ActivityId` is not in `live`,
    /// and remove its map entry. Called at the end of each rebuild so
    /// closed Activities release their GPU/CPU resources.
    pub(crate) fn prune(&mut self, commands: &mut Commands, live: &HashSet<ActivityId>) {
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

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entities.len()
    }

    #[cfg(test)]
    pub(crate) fn get(&self, id: &ActivityId) -> Option<Entity> {
        self.entities.get(id).copied()
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
}
