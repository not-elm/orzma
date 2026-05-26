//! Per-Session UI rebuild scoped by `MultiplexerService` session-epoch
//! table. Replaces `rebuild_structure_on_change`. Only rebuilds sessions
//! whose epoch advanced since the last run (tracked in `Local<HashMap<...>>`).

use crate::multiplexer::{AttachedSession, Multiplexer, SessionEntityId, SessionUiSubtree};
use crate::ui::registry::ActivityEntityRegistry;
use crate::ui::{ActivityHostNode, StructuralNode};
use bevy::ecs::change_detection::DetectChanges;
use bevy::prelude::*;
use ozmux_multiplexer::{ActivityId, Cell, SessionId};
use std::collections::{HashMap, HashSet};

/// Rebuilds the UI subtree of every Session whose epoch advanced since the
/// last run. Skips sessions whose epoch is unchanged. The rebuild walks
/// `session.cells` and replaces every `StructuralNode` descendant of the
/// session's `SessionUiSubtree` root — Activity hosts are preserved via
/// `ActivityEntityRegistry` and re-parented.
pub(crate) fn rebuild_session_ui_on_data_change(
    mut commands: Commands,
    mut last_epochs: Local<HashMap<SessionId, u64>>,
    mut registry: ResMut<ActivityEntityRegistry>,
    mux: Res<Multiplexer>,
    sessions_q: Query<(&SessionEntityId, &SessionUiSubtree, Has<AttachedSession>)>,
    structural_q: Query<(Entity, Option<&ChildOf>), With<StructuralNode>>,
    activity_hosts_q: Query<(Entity, &ActivityHostNode)>,
    children_q: Query<&Children>,
    ui_font: Option<Res<crate::font::TerminalUiFont>>,
) {
    if !mux.is_changed() {
        return;
    }

    let ui_font_handle = ui_font.as_deref().map(|f| f.0.clone()).unwrap_or_default();

    // Collect every Activity that exists in the multiplexer domain across
    // ALL sessions — pruning live set.
    let live_activity_ids: HashSet<ActivityId> = mux
        .sessions
        .values()
        .flat_map(|s| s.pane_ids().filter_map(|pid| s.pane(pid).ok()))
        .flat_map(|p| p.activity_ids().cloned())
        .collect();

    for (session_eid, subtree, is_attached) in sessions_q.iter() {
        let sid = session_eid.0;
        let cur_epoch = mux.epoch_of(&sid);
        let prev = last_epochs.get(&sid).copied().unwrap_or(0);
        if cur_epoch <= prev {
            continue;
        }

        let Some(session) = mux.sessions.get(&sid) else {
            continue;
        };

        descend_and_detach_hosts(&mut commands, subtree.0, &children_q, &activity_hosts_q);

        descend_and_despawn_structural(&mut commands, subtree.0, &children_q, &structural_q);

        match session.cells.cell(&session.root_cell) {
            Ok(Cell::Root(root)) => {
                crate::ui::layout::build_cell_recursive(
                    &mut commands,
                    subtree.0,
                    session,
                    &root.child,
                    &mut registry,
                    subtree.0,
                    &ui_font_handle,
                );
            }
            Ok(_) => {
                tracing::warn!(
                    target: "ozmux_gui::ui",
                    session = ?sid,
                    "session.root_cell is not Cell::Root",
                );
            }
            Err(err) => {
                tracing::warn!(
                    target: "ozmux_gui::ui",
                    session = ?sid,
                    ?err,
                    "session.root_cell missing",
                );
            }
        }

        last_epochs.insert(sid, cur_epoch);
        let _ = is_attached;
    }

    registry.prune(&mut commands, &live_activity_ids);
}

fn descend_and_detach_hosts(
    commands: &mut Commands,
    root: Entity,
    children_q: &Query<&Children>,
    activity_hosts_q: &Query<(Entity, &ActivityHostNode)>,
) {
    let mut stack = vec![root];
    while let Some(e) = stack.pop() {
        if activity_hosts_q.get(e).is_ok() {
            commands.entity(e).remove::<ChildOf>();
            continue;
        }
        if let Ok(children) = children_q.get(e) {
            for c in children.iter() {
                stack.push(c);
            }
        }
    }
}

fn descend_and_despawn_structural(
    commands: &mut Commands,
    root: Entity,
    children_q: &Query<&Children>,
    structural_q: &Query<(Entity, Option<&ChildOf>), With<StructuralNode>>,
) {
    let mut stack = vec![root];
    while let Some(e) = stack.pop() {
        if let Ok(children) = children_q.get(e) {
            for c in children.iter() {
                stack.push(c);
            }
        }
    }
    let mut to_despawn = vec![];
    let mut stack2 = vec![root];
    while let Some(e) = stack2.pop() {
        if let Ok(children) = children_q.get(e) {
            for c in children.iter() {
                stack2.push(c);
            }
        }
        if structural_q.get(e).is_ok() && e != root {
            to_despawn.push(e);
        }
    }
    for e in to_despawn {
        commands.entity(e).try_despawn();
    }
}
