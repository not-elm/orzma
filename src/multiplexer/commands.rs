//! Pure(ish) functions that apply a `configs::Action` to the multiplexer
//! via `MultiplexerCommands`. Called by the shortcut dispatcher in
//! `src/input.rs`.
//!
//! `apply()` handles in-session mutations only. Actions that mint
//! sessions (`NewSession`) or move the `AttachedSession` marker between
//! Session entities (`FocusSession`, `FocusSessionNumber`) are
//! dispatched in Bevy-side systems in `src/input.rs` because they
//! require entity-spawning side effects.

use bevy::prelude::*;
use ozmux_configs::shortcuts::{
    Action, ActivityOffset as ConfigActivityOffset, Direction as ConfigDirection, SplitDirection,
    SwapOffset as ConfigSwapOffset,
};
use ozmux_multiplexer::{
    ActivityKind, CycleDirection, MultiplexerCommands, PaneDirection, Side, SplitOrientation,
    SwapOffset,
};

/// Applies `action` to the multiplexer for the given session entity.
/// Returns `true` if state was mutated, `false` otherwise.
///
/// Actions handled outside `apply()` (`NewSession`, `FocusSession`,
/// `FocusSessionNumber`) are short-circuited to `false` because their
/// side effects are entity-spawning / marker-moving operations the Bevy
/// dispatcher performs directly.
pub fn apply(action: Action, mux: &mut MultiplexerCommands, session: Entity) -> bool {
    match action {
        Action::SplitPane { direction } => apply_split(mux, session, split_orientation(direction)),
        Action::NewTerminalActivity => apply_new_activity(mux, session),
        Action::FocusPane { direction } => apply_focus_pane(mux, session, focus_direction(direction)),
        Action::FocusActivity { offset } => match cycle_direction(offset) {
            Some(direction) => apply_focus_activity(mux, session, direction),
            None => {
                tracing::debug!(
                    target: "ozmux_gui::commands",
                    "FocusActivity::Last not yet implemented"
                );
                false
            }
        },
        Action::SwapPane { offset } => apply_swap_pane(mux, session, swap_offset(offset)),
        Action::ClosePane => apply_close_pane(mux, session),
        Action::CloseActivity => apply_close_activity(mux, session),
        Action::NewSession | Action::FocusSession { .. } | Action::FocusSessionNumber { .. } => false,
        other => {
            tracing::debug!(target: "ozmux_gui::commands", ?other, "shortcut action not yet implemented");
            false
        }
    }
}

fn split_orientation(d: SplitDirection) -> SplitOrientation {
    match d {
        SplitDirection::Horizontal => SplitOrientation::Horizontal,
        SplitDirection::Vertical => SplitOrientation::Vertical,
    }
}

fn focus_direction(d: ConfigDirection) -> PaneDirection {
    match d {
        ConfigDirection::Up => PaneDirection::Up,
        ConfigDirection::Down => PaneDirection::Down,
        ConfigDirection::Left => PaneDirection::Left,
        ConfigDirection::Right => PaneDirection::Right,
    }
}

fn swap_offset(o: ConfigSwapOffset) -> SwapOffset {
    match o {
        ConfigSwapOffset::Prev => SwapOffset::Prev,
        ConfigSwapOffset::Next => SwapOffset::Next,
    }
}

fn cycle_direction(o: ConfigActivityOffset) -> Option<CycleDirection> {
    match o {
        ConfigActivityOffset::Next => Some(CycleDirection::Next),
        ConfigActivityOffset::Prev => Some(CycleDirection::Prev),
        ConfigActivityOffset::Last => None,
    }
}

fn apply_split(mux: &mut MultiplexerCommands, session: Entity, orientation: SplitOrientation) -> bool {
    let Some(active_pane) = read_active_pane(mux, session) else {
        tracing::warn!(target: "ozmux_gui::commands", ?session, "Split: session vanished");
        return false;
    };
    match mux.split_pane(active_pane, Side::After, orientation) {
        Ok(_) => true,
        Err(err) => {
            tracing::warn!(target: "ozmux_gui::commands", ?err, "split_pane failed");
            false
        }
    }
}

fn apply_new_activity(mux: &mut MultiplexerCommands, session: Entity) -> bool {
    let Some(active_pane) = read_active_pane(mux, session) else {
        tracing::warn!(target: "ozmux_gui::commands", ?session, "NewActivity: session vanished");
        return false;
    };
    let new_activity = mux.add_activity(active_pane, ActivityKind::Terminal);
    if let Err(err) = mux.set_active_activity(active_pane, new_activity) {
        tracing::warn!(target: "ozmux_gui::commands", ?err, "NewActivity: set_active_activity failed");
        return false;
    }
    true
}

fn apply_focus_pane(mux: &mut MultiplexerCommands, session: Entity, direction: PaneDirection) -> bool {
    // TODO: pane_in_direction needs layout access through MultiplexerCommands.
    // Deferred to follow-up task — direction-based focus requires reading
    // LayoutCells and calling ozmux_multiplexer::direction::pane_in_direction,
    // which is not yet surfaced through MultiplexerCommands.
    let _ = (mux, session, direction);
    tracing::debug!(target: "ozmux_gui::commands", "FocusPane: deferred to follow-up task");
    false
}

fn apply_focus_activity(
    mux: &mut MultiplexerCommands,
    session: Entity,
    direction: CycleDirection,
) -> bool {
    let Some(active_pane) = read_active_pane(mux, session) else {
        tracing::warn!(target: "ozmux_gui::commands", ?session, "FocusActivity: session vanished");
        return false;
    };
    let Some(active_activity) = read_active_activity(mux, active_pane) else {
        tracing::warn!(target: "ozmux_gui::commands", ?active_pane, "FocusActivity: pane vanished");
        return false;
    };

    // Collect into Vec to allow indexing and to release the iterator borrow
    // before calling set_active_activity.
    let activities: Vec<Entity> = mux.activities_of_pane(active_pane).collect();
    if activities.len() < 2 {
        return false;
    }

    let i = activities.iter().position(|a| *a == active_activity).unwrap_or(0);
    let len = activities.len() as isize;
    let delta: isize = match direction {
        CycleDirection::Next => 1,
        CycleDirection::Prev => -1,
    };
    let j = ((i as isize + delta).rem_euclid(len)) as usize;
    let target = activities[j];

    match mux.set_active_activity(active_pane, target) {
        Ok(()) => true,
        Err(err) => {
            tracing::warn!(target: "ozmux_gui::commands", ?err, "FocusActivity failed");
            false
        }
    }
}

fn apply_swap_pane(mux: &mut MultiplexerCommands, session: Entity, offset: SwapOffset) -> bool {
    let Some(active_pane) = read_active_pane(mux, session) else {
        tracing::warn!(target: "ozmux_gui::commands", ?session, "SwapPane: session vanished");
        return false;
    };
    match mux.swap_pane(active_pane, offset) {
        Ok(ozmux_multiplexer::SwapOutcome::Swapped { .. }) => true,
        Ok(ozmux_multiplexer::SwapOutcome::NoOp) => false,
        Err(err) => {
            tracing::warn!(target: "ozmux_gui::commands", ?err, "swap_pane failed");
            false
        }
    }
}

fn apply_close_pane(mux: &mut MultiplexerCommands, session: Entity) -> bool {
    let Some(active_pane) = read_active_pane(mux, session) else {
        tracing::warn!(target: "ozmux_gui::commands", ?session, "ClosePane: session vanished");
        return false;
    };
    match mux.close_pane(active_pane) {
        Ok(()) => true,
        Err(err) => {
            tracing::warn!(target: "ozmux_gui::commands", ?err, "ClosePane failed");
            false
        }
    }
}

fn apply_close_activity(mux: &mut MultiplexerCommands, session: Entity) -> bool {
    let Some(active_pane) = read_active_pane(mux, session) else {
        tracing::warn!(target: "ozmux_gui::commands", ?session, "CloseActivity: session vanished");
        return false;
    };
    let Some(active_activity) = read_active_activity(mux, active_pane) else {
        tracing::warn!(target: "ozmux_gui::commands", ?active_pane, "CloseActivity: pane vanished");
        return false;
    };

    let activity_count = mux.activities_of_pane(active_pane).count();
    if activity_count > 1 {
        // TODO: despawn a single Activity without closing the Pane. Requires
        // a `despawn_activity` method on MultiplexerCommands (or equivalent)
        // that handles ActiveActivity repointing. Deferred to Task 16.
        tracing::debug!(target: "ozmux_gui::commands", "CloseActivity (multi-activity): deferred to Task 16");
        let _ = active_activity;
        false
    } else {
        match mux.close_pane(active_pane) {
            Ok(()) => true,
            Err(err) => {
                tracing::warn!(target: "ozmux_gui::commands", ?err, "CloseActivity (single): close_pane failed");
                false
            }
        }
    }
}

fn read_active_pane(mux: &MultiplexerCommands, session: Entity) -> Option<Entity> {
    mux.sessions_active_pane(session)
}

fn read_active_activity(mux: &MultiplexerCommands, pane: Entity) -> Option<Entity> {
    mux.panes_active_activity(pane)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use ozmux_configs::shortcuts::{ActivityOffset, Direction, SessionOffset, SwapOffset as CfgSwapOffset};
    use ozmux_multiplexer::{MultiplexerPlugin, SessionMarker, ActivePane, ActiveActivity};

    fn setup_app() -> bevy::app::App {
        let mut app = bevy::app::App::new();
        app.add_plugins(MultiplexerPlugin);
        app
    }

    fn bootstrap_session(world: &mut bevy::ecs::world::World) -> Entity {
        world
            .run_system_once(|mut mux: MultiplexerCommands| {
                mux.create_session(Some("test".into())).session
            })
            .unwrap()
    }

    #[test]
    fn new_session_action_short_circuits_in_apply() {
        let mut app = setup_app();
        let session = bootstrap_session(app.world_mut());
        let mutated = app
            .world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                apply(Action::NewSession, &mut mux, session)
            })
            .unwrap();
        assert!(!mutated);
    }

    #[test]
    fn focus_session_action_short_circuits_in_apply() {
        let mut app = setup_app();
        let session = bootstrap_session(app.world_mut());
        let mutated = app
            .world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                apply(
                    Action::FocusSession { offset: SessionOffset::Next },
                    &mut mux,
                    session,
                )
            })
            .unwrap();
        assert!(!mutated);
    }

    #[test]
    fn focus_session_number_action_short_circuits_in_apply() {
        let mut app = setup_app();
        let session = bootstrap_session(app.world_mut());
        let mutated = app
            .world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                apply(Action::FocusSessionNumber { index: 0 }, &mut mux, session)
            })
            .unwrap();
        assert!(!mutated);
    }

    #[test]
    fn split_pane_horizontal_action_adds_pane_to_session() {
        let mut app = setup_app();
        let session = bootstrap_session(app.world_mut());
        let mutated = app
            .world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                apply(
                    Action::SplitPane { direction: SplitDirection::Horizontal },
                    &mut mux,
                    session,
                )
            })
            .unwrap();
        assert!(mutated);
        app.world_mut().flush();
        let pane_count = app
            .world_mut()
            .run_system_once(move |mux: MultiplexerCommands| {
                mux.panes_of_session(session).count()
            })
            .unwrap();
        assert_eq!(pane_count, 2);
    }

    #[test]
    fn split_pane_vertical_action_adds_pane_to_session() {
        let mut app = setup_app();
        let session = bootstrap_session(app.world_mut());
        let mutated = app
            .world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                apply(
                    Action::SplitPane { direction: SplitDirection::Vertical },
                    &mut mux,
                    session,
                )
            })
            .unwrap();
        assert!(mutated);
        app.world_mut().flush();
        let pane_count = app
            .world_mut()
            .run_system_once(move |mux: MultiplexerCommands| {
                mux.panes_of_session(session).count()
            })
            .unwrap();
        assert_eq!(pane_count, 2);
    }

    #[test]
    fn split_pane_promotes_new_pane_to_active() {
        let mut app = setup_app();
        let session = bootstrap_session(app.world_mut());
        let original_active = app
            .world()
            .get::<ActivePane>(session)
            .map(|a| a.0)
            .unwrap();
        app.world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                apply(
                    Action::SplitPane { direction: SplitDirection::Horizontal },
                    &mut mux,
                    session,
                )
            })
            .unwrap();
        let new_active = app
            .world()
            .get::<ActivePane>(session)
            .map(|a| a.0)
            .unwrap();
        assert_ne!(new_active, original_active);
    }

    #[test]
    fn new_terminal_activity_adds_and_activates_activity_on_active_pane() {
        let mut app = setup_app();
        let session = bootstrap_session(app.world_mut());
        let active_pane = app
            .world()
            .get::<ActivePane>(session)
            .map(|a| a.0)
            .unwrap();
        let mutated = app
            .world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                apply(Action::NewTerminalActivity, &mut mux, session)
            })
            .unwrap();
        assert!(mutated);
        app.world_mut().flush();
        let activity_count = app
            .world_mut()
            .run_system_once(move |mux: MultiplexerCommands| {
                mux.activities_of_pane(active_pane).count()
            })
            .unwrap();
        assert_eq!(activity_count, 2);
    }

    #[test]
    fn unimplemented_action_returns_false_without_state_change() {
        let mut app = setup_app();
        let session = bootstrap_session(app.world_mut());
        let mutated = app
            .world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                apply(Action::ZoomPane, &mut mux, session)
            })
            .unwrap();
        assert!(!mutated);
    }

    #[test]
    fn focus_pane_in_single_pane_session_returns_false() {
        let mut app = setup_app();
        let session = bootstrap_session(app.world_mut());
        let mutated = app
            .world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                apply(
                    Action::FocusPane { direction: Direction::Right },
                    &mut mux,
                    session,
                )
            })
            .unwrap();
        // NOTE: FocusPane is stubbed — always returns false until Task 14.
        assert!(!mutated);
    }

    #[test]
    fn swap_pane_in_single_pane_session_returns_false() {
        let mut app = setup_app();
        let session = bootstrap_session(app.world_mut());
        let mutated = app
            .world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                apply(
                    Action::SwapPane { offset: CfgSwapOffset::Prev },
                    &mut mux,
                    session,
                )
            })
            .unwrap();
        assert!(!mutated);
    }

    #[test]
    fn close_pane_action_removes_pane_and_promotes_survivor() {
        let mut app = setup_app();
        let session = bootstrap_session(app.world_mut());
        app.world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                apply(
                    Action::SplitPane { direction: SplitDirection::Horizontal },
                    &mut mux,
                    session,
                )
            })
            .unwrap();
        app.world_mut().flush();
        let active_before = app
            .world()
            .get::<ActivePane>(session)
            .map(|a| a.0)
            .unwrap();
        let mutated = app
            .world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                apply(Action::ClosePane, &mut mux, session)
            })
            .unwrap();
        assert!(mutated);
        app.world_mut().flush();
        let active_after = app
            .world()
            .get::<ActivePane>(session)
            .map(|a| a.0)
            .unwrap();
        assert_ne!(active_after, active_before, "active pane should change after close");
    }

    #[test]
    fn close_pane_in_single_pane_session_returns_false() {
        let mut app = setup_app();
        let session = bootstrap_session(app.world_mut());
        let mutated = app
            .world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                apply(Action::ClosePane, &mut mux, session)
            })
            .unwrap();
        assert!(!mutated);
    }

    #[test]
    fn close_activity_in_single_activity_pane_returns_false() {
        let mut app = setup_app();
        let session = bootstrap_session(app.world_mut());
        let mutated = app
            .world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                apply(Action::CloseActivity, &mut mux, session)
            })
            .unwrap();
        assert!(!mutated);
    }

    #[test]
    fn focus_activity_next_advances_active_activity() {
        let mut app = setup_app();
        let session = bootstrap_session(app.world_mut());
        let active_pane = app.world().get::<ActivePane>(session).map(|a| a.0).unwrap();
        app.world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                apply(Action::NewTerminalActivity, &mut mux, session)
            })
            .unwrap();
        app.world_mut().flush();
        let active_before = app
            .world()
            .get::<ActiveActivity>(active_pane)
            .map(|a| a.0)
            .unwrap();
        // Reset to first activity so we can test Next advances it.
        let first_activity = app
            .world_mut()
            .run_system_once(move |mux: MultiplexerCommands| {
                mux.activities_of_pane(active_pane)
                    .find(|a| *a != active_before)
            })
            .unwrap()
            .expect("second activity exists");
        app.world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.set_active_activity(active_pane, first_activity).unwrap();
            })
            .unwrap();
        let mutated = app
            .world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                apply(
                    Action::FocusActivity { offset: ActivityOffset::Next },
                    &mut mux,
                    session,
                )
            })
            .unwrap();
        assert!(mutated);
        let active_after = app
            .world()
            .get::<ActiveActivity>(active_pane)
            .map(|a| a.0)
            .unwrap();
        assert_ne!(active_after, first_activity);
    }

    #[test]
    fn focus_activity_in_single_activity_pane_returns_false() {
        let mut app = setup_app();
        let session = bootstrap_session(app.world_mut());
        let mutated = app
            .world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                apply(
                    Action::FocusActivity { offset: ActivityOffset::Next },
                    &mut mux,
                    session,
                )
            })
            .unwrap();
        assert!(!mutated);
    }

    #[test]
    fn focus_activity_last_returns_false_without_state_change() {
        let mut app = setup_app();
        let session = bootstrap_session(app.world_mut());
        let mutated = app
            .world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                apply(
                    Action::FocusActivity { offset: ActivityOffset::Last },
                    &mut mux,
                    session,
                )
            })
            .unwrap();
        assert!(!mutated);
    }

    #[test]
    fn apply_on_vanished_session_returns_false() {
        let mut app = setup_app();
        let bogus = app.world_mut().spawn(SessionMarker).id();
        app.world_mut().despawn(bogus);
        app.world_mut().flush();
        let mutated = app
            .world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                apply(Action::ClosePane, &mut mux, bogus)
            })
            .unwrap();
        assert!(!mutated);
    }
}
