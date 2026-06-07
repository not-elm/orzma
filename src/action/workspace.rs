//! Workspace-lifecycle shortcut actions dispatched as `EntityEvent`s.
//!
//! `NewWorkspaceActionEvent` and `FocusWorkspaceActionEvent` are triggered by
//! the keyboard dispatcher (`crate::input::execute_action`) and handled by
//! the observers below. The observers re-query the live `AttachedWorkspace`
//! marker rather than trusting the event's target, so two same-frame
//! triggers preserve the single-holder invariant (Bevy flushes each
//! observer's commands before the next queued trigger runs).

use bevy::prelude::*;
#[cfg(not(feature = "thin-client"))]
use ozmux_multiplexer::MultiplexerCommands;
use ozmux_multiplexer::{AttachedWorkspace, WorkspaceCreatedAt, WorkspaceMarker};

/// Bevy Plugin that registers the workspace-action observers.
pub struct OzmuxWorkspaceActionPlugin;

impl Plugin for OzmuxWorkspaceActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_new_workspace)
            .add_observer(apply_focus_workspace);
    }
}

/// Request to mint a new workspace and attach it. Triggered by
/// `ShortcutAction::NewWorkspace`.
#[derive(EntityEvent, Debug)]
pub struct NewWorkspaceActionEvent {
    /// The workspace attached at dispatch time (trigger target only; the
    /// observer re-queries the live marker).
    #[event_target]
    pub workspace: Entity,
}

/// Request to move workspace focus. Triggered by
/// `ShortcutAction::FocusWorkspace` and `ShortcutAction::FocusWorkspaceNumber`.
#[derive(EntityEvent, Debug)]
pub struct FocusWorkspaceActionEvent {
    /// The workspace attached at dispatch time (trigger target only; the
    /// observer re-queries the live marker).
    #[event_target]
    pub workspace: Entity,
    /// Which workspace to focus.
    pub target: FocusWorkspaceTarget,
}

/// Selector for `FocusWorkspaceActionEvent`, unifying `FocusWorkspace{offset}` and `FocusWorkspaceNumber{index}`. `Debug` is required because
/// `FocusWorkspaceActionEvent` derives `Debug`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusWorkspaceTarget {
    Next,
    Prev,
    Last,
    Number(u8),
}

fn apply_new_workspace(
    _trigger: On<NewWorkspaceActionEvent>,
    #[cfg(not(feature = "thin-client"))] mut mux: MultiplexerCommands,
    #[cfg(feature = "thin-client")] _conn: bevy::ecs::system::NonSendMut<
        crate::thin_client::ThinClientConn,
    >,
    mut commands: Commands,
    attached_workspace: Query<Entity, (With<WorkspaceMarker>, With<AttachedWorkspace>)>,
) {
    #[cfg(not(feature = "thin-client"))]
    {
        match attached_workspace.single() {
            Ok(previous_attached) => {
                tracing::debug!(
                    target: "ozmux_gui::action",
                    ?previous_attached,
                    "apply_new_workspace: queued AttachedWorkspace remove from previous"
                );
                commands
                    .entity(previous_attached)
                    .remove::<AttachedWorkspace>();
            }
            Err(err) => {
                tracing::debug!(
                    target: "ozmux_gui::action",
                    ?err,
                    "apply_new_workspace: no single previously-attached workspace (skipping remove)"
                );
            }
        }
        let new_workspace = mux.spawn_attached_workspace();
        tracing::debug!(
            target: "ozmux_gui::action",
            ?new_workspace,
            "apply_new_workspace: queued spawn of new attached workspace"
        );
    }
    #[cfg(feature = "thin-client")]
    {
        // TODO(T5): send ClientMessage::NewWorkspace over the wire.
        let _ = (&mut commands, &attached_workspace);
    }
}

fn apply_focus_workspace(
    trigger: On<FocusWorkspaceActionEvent>,
    #[cfg(not(feature = "thin-client"))] mut mux: MultiplexerCommands,
    #[cfg(feature = "thin-client")] _conn: bevy::ecs::system::NonSendMut<
        crate::thin_client::ThinClientConn,
    >,
    mut commands: Commands,
    workspaces: Query<(Entity, Option<&WorkspaceCreatedAt>), With<WorkspaceMarker>>,
    attached_workspace: Query<Entity, (With<WorkspaceMarker>, With<AttachedWorkspace>)>,
) {
    #[cfg(not(feature = "thin-client"))]
    {
        let mut pairs: Vec<(Entity, u32)> = workspaces
            .iter()
            .map(|(e, created)| (e, created.map(|c| c.0).unwrap_or(u32::MAX)))
            .collect();
        if pairs.len() < 2 {
            return;
        }
        pairs.sort_by_key(|(_, c)| *c);
        let entries: Vec<Entity> = pairs.into_iter().map(|(e, _)| e).collect();

        let Ok(current_entity) = attached_workspace.single() else {
            return;
        };
        let Some(current_idx) = entries.iter().position(|e| *e == current_entity) else {
            return;
        };

        let target_idx = match trigger.event().target {
            FocusWorkspaceTarget::Next => (current_idx + 1) % entries.len(),
            FocusWorkspaceTarget::Prev => current_idx.checked_sub(1).unwrap_or(entries.len() - 1),
            FocusWorkspaceTarget::Last => {
                tracing::debug!(
                    target: "ozmux_gui::action",
                    "FocusWorkspace::Last not yet implemented"
                );
                return;
            }
            FocusWorkspaceTarget::Number(index) => {
                let i = index as usize;
                if i >= entries.len() {
                    return;
                }
                i
            }
        };

        let target_entity = entries[target_idx];
        if target_entity == current_entity {
            return;
        }

        commands
            .entity(current_entity)
            .remove::<AttachedWorkspace>();
        commands.entity(target_entity).insert(AttachedWorkspace);
        let _ = mux.select_workspace(target_entity);
    }
    #[cfg(feature = "thin-client")]
    {
        // TODO(T5): send ClientMessage::FocusWorkspace over the wire.
        let _ = (&trigger, &mut commands, &workspaces, &attached_workspace);
    }
}

#[cfg(all(test, not(feature = "thin-client")))]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use ozmux_multiplexer::{MultiplexerPlugin, WorkspaceUiSubtree};

    /// Builds an app with the multiplexer + workspace-action observers and a
    /// single attached "default" workspace (no `WorkspaceCreatedAt`, mirroring
    /// the pre-counter bootstrap workspace). Returns the workspace entity.
    fn setup_app() -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(MultiplexerPlugin)
            .add_plugins(OzmuxWorkspaceActionPlugin);
        app.update();
        let workspace = app
            .world_mut()
            .query_filtered::<Entity, With<WorkspaceMarker>>()
            .iter(app.world())
            .next()
            .expect("Mux-seeded workspace must exist after Startup");
        app.world_mut()
            .entity_mut(workspace)
            .insert((Name::new("default"), AttachedWorkspace));
        (app, workspace)
    }

    fn count_workspace_entities(app: &mut App) -> usize {
        app.world_mut()
            .query_filtered::<Entity, With<WorkspaceMarker>>()
            .iter(app.world())
            .count()
    }

    fn count_attached_workspace_entities(app: &mut App) -> usize {
        app.world_mut()
            .query_filtered::<Entity, (With<WorkspaceMarker>, With<AttachedWorkspace>)>()
            .iter(app.world())
            .count()
    }

    fn attached_now(app: &mut App) -> Entity {
        app.world_mut()
            .query_filtered::<Entity, (With<WorkspaceMarker>, With<AttachedWorkspace>)>()
            .iter(app.world())
            .next()
            .unwrap()
    }

    #[test]
    fn plugin_builds_without_panic() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(MultiplexerPlugin)
            .add_plugins(OzmuxWorkspaceActionPlugin);
        app.update();
    }

    #[test]
    fn new_workspace_event_spawns_entity_with_subtree_and_moves_marker() {
        let (mut app, bootstrap) = setup_app();
        assert_eq!(count_workspace_entities(&mut app), 1);
        assert_eq!(count_attached_workspace_entities(&mut app), 1);

        app.world_mut().trigger(NewWorkspaceActionEvent {
            workspace: bootstrap,
        });
        app.update();

        assert_eq!(count_workspace_entities(&mut app), 2);
        assert_eq!(count_attached_workspace_entities(&mut app), 1);
        let new_attached = attached_now(&mut app);
        assert_ne!(new_attached, bootstrap);
        assert!(
            app.world()
                .get::<WorkspaceUiSubtree>(new_attached)
                .is_some(),
            "new attached workspace must carry a WorkspaceUiSubtree pointer",
        );
    }

    #[test]
    fn two_new_workspace_events_same_frame_keep_marker_invariant() {
        // §6.2: queue two triggers in ONE system run (mirroring
        // execute_action's keyboard loop), flush once. Bevy applies the
        // first observer's marker move before the second observer runs, so
        // exactly one AttachedWorkspace survives and TWO new workspaces exist.
        let (mut app, bootstrap) = setup_app();
        assert_eq!(count_workspace_entities(&mut app), 1);

        app.world_mut()
            .run_system_once(move |mut commands: Commands| {
                commands.trigger(NewWorkspaceActionEvent {
                    workspace: bootstrap,
                });
                commands.trigger(NewWorkspaceActionEvent {
                    workspace: bootstrap,
                });
            })
            .unwrap();
        app.update();

        assert_eq!(
            count_attached_workspace_entities(&mut app),
            1,
            "exactly one AttachedWorkspace after two same-frame NewWorkspace triggers",
        );
        assert_eq!(
            count_workspace_entities(&mut app),
            3,
            "two same-frame NewWorkspace triggers must create two new workspaces",
        );
    }

    #[test]
    fn new_workspace_event_uses_monotonic_name_and_created_at() {
        let (mut app, bootstrap) = setup_app();
        app.world_mut().trigger(NewWorkspaceActionEvent {
            workspace: bootstrap,
        });
        app.update();
        let first_new = attached_now(&mut app);
        app.world_mut().trigger(NewWorkspaceActionEvent {
            workspace: first_new,
        });
        app.update();

        let world = app.world_mut();
        let mut created = world
            .query_filtered::<&WorkspaceCreatedAt, With<WorkspaceMarker>>()
            .iter(world)
            .map(|c| c.0)
            .collect::<Vec<u32>>();
        created.sort_unstable();
        assert_eq!(created, vec![1, 2]);

        let mut names = world
            .query_filtered::<&Name, With<WorkspaceMarker>>()
            .iter(world)
            .map(|n| n.as_str().to_owned())
            .collect::<Vec<String>>();
        names.sort();
        assert_eq!(names, vec!["default", "workspace1", "workspace2"]);
    }

    #[test]
    fn focus_workspace_number_targets_sorted_index() {
        let (mut app, bootstrap) = setup_app();
        // Add a second workspace via the event; it gets WorkspaceCreatedAt(1),
        // so sort order is [workspace1(1), default(u32::MAX)].
        app.world_mut().trigger(NewWorkspaceActionEvent {
            workspace: bootstrap,
        });
        app.update();
        let workspace1 = attached_now(&mut app);

        // Index 1 → the "default" workspace (sorts last because it has no
        // WorkspaceCreatedAt). The current attached is workspace1 (index 0).
        app.world_mut().trigger(FocusWorkspaceActionEvent {
            workspace: workspace1,
            target: FocusWorkspaceTarget::Number(1),
        });
        app.update();

        assert_eq!(count_attached_workspace_entities(&mut app), 1);
        let focused = attached_now(&mut app);
        assert_eq!(
            focused, bootstrap,
            "Number(1) targets the default workspace"
        );
    }

    #[test]
    fn focus_workspace_next_moves_marker_to_other_workspace() {
        let (mut app, bootstrap) = setup_app();
        app.world_mut().trigger(NewWorkspaceActionEvent {
            workspace: bootstrap,
        });
        app.update();
        let workspace1 = attached_now(&mut app);

        app.world_mut().trigger(FocusWorkspaceActionEvent {
            workspace: workspace1,
            target: FocusWorkspaceTarget::Next,
        });
        app.update();

        assert_eq!(count_attached_workspace_entities(&mut app), 1);
        assert_ne!(
            attached_now(&mut app),
            workspace1,
            "Next must move the marker off the currently-attached workspace",
        );
    }

    #[test]
    fn focus_workspace_prev_moves_marker_to_other_workspace() {
        let (mut app, bootstrap) = setup_app();
        app.world_mut().trigger(NewWorkspaceActionEvent {
            workspace: bootstrap,
        });
        app.update();
        let workspace1 = attached_now(&mut app);

        app.world_mut().trigger(FocusWorkspaceActionEvent {
            workspace: workspace1,
            target: FocusWorkspaceTarget::Prev,
        });
        app.update();

        assert_eq!(count_attached_workspace_entities(&mut app), 1);
        assert_ne!(
            attached_now(&mut app),
            workspace1,
            "Prev must move the marker off the currently-attached workspace",
        );
    }

    #[test]
    fn focus_workspace_number_out_of_bounds_is_noop() {
        let (mut app, bootstrap) = setup_app();
        app.world_mut().trigger(NewWorkspaceActionEvent {
            workspace: bootstrap,
        });
        app.update();
        let workspace1 = attached_now(&mut app);

        app.world_mut().trigger(FocusWorkspaceActionEvent {
            workspace: workspace1,
            target: FocusWorkspaceTarget::Number(99),
        });
        app.update();

        assert_eq!(count_attached_workspace_entities(&mut app), 1);
        assert_eq!(
            attached_now(&mut app),
            workspace1,
            "out-of-bounds Number must leave the marker unchanged",
        );
    }
}
