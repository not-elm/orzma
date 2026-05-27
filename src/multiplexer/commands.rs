//! Pure (Bevy-independent) functions that apply a `configs::Action` to the
//! domain `MultiplexerService`. Called by the shortcut dispatcher.
//!
//! `apply()` handles in-session mutations only. Actions that mint sessions
//! (`NewSession`) or move the `AttachedSession` marker between session
//! entities (`FocusSession`, `FocusSessionNumber`) are dispatched in
//! Bevy-side systems in `src/input.rs` because they require entity-level
//! side effects beyond a pure `MultiplexerService` mutation.

use ozmux_configs::shortcuts::{
    Action, ActivityOffset as ConfigActivityOffset, Direction as ConfigDirection, SplitDirection,
    SwapOffset as ConfigSwapOffset,
};
use ozmux_multiplexer::{
    Activity, ActivityId, CycleDirection, MultiplexerResult, MultiplexerService, PaneDirection,
    PaneId, SessionId, SetActiveOutcome, Side, SplitOrientation, SwapOffset, SwapOutcome,
};

/// Applies `action` to the domain `MultiplexerService` for the given session.
/// Returns `true` if the domain state was mutated, `false` otherwise.
///
/// Actions handled outside `apply()` (NewSession, FocusSession,
/// FocusSessionNumber) are short-circuited here to `false` because their
/// side effects are entity-spawning / marker-moving operations the Bevy
/// dispatcher performs directly.
pub fn apply(action: Action, mux: &mut MultiplexerService, session: SessionId) -> bool {
    match action {
        Action::SplitPane { direction } => apply_split(mux, &session, split_orientation(direction)),
        Action::NewTerminalActivity => apply_new_activity(mux, &session),
        Action::FocusPane { direction } => {
            apply_focus_pane(mux, &session, focus_direction(direction))
        }
        Action::FocusActivity { offset } => match cycle_direction(offset) {
            Some(direction) => apply_focus_activity(mux, &session, direction),
            None => {
                tracing::debug!(
                    target: "ozmux_gui::commands",
                    "FocusActivity::Last not yet implemented"
                );
                false
            }
        },
        Action::SwapPane { offset } => apply_swap_pane(mux, &session, swap_offset(offset)),
        Action::ClosePane => apply_close_pane(mux, &session),
        Action::CloseActivity => apply_close_activity(mux, &session),
        // Handled in src/input.rs (Bevy-side dispatchers).
        Action::NewSession | Action::FocusSession { .. } | Action::FocusSessionNumber { .. } => {
            false
        }
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

fn apply_split(
    mux: &mut MultiplexerService,
    session: &SessionId,
    orientation: SplitOrientation,
) -> bool {
    let session_id = *session;
    let active_pane = match mux.sessions.get(&session_id) {
        Some(s) => s.active_pane.clone(),
        None => {
            tracing::warn!(target: "ozmux_gui::commands", ?session_id, "Split: session vanished");
            return false;
        }
    };
    let new_pane_id = PaneId::new();
    let new_activity = Activity::terminal(ActivityId::new());
    let outcome = mux.with_session(&session_id, |s| {
        s.split_pane(
            &active_pane,
            new_pane_id.clone(),
            new_activity,
            Side::After,
            orientation,
        )
    });
    match outcome {
        Some(Ok(())) => {
            mux.pane_owner_session.insert(new_pane_id, session_id);
            true
        }
        Some(Err(err)) => {
            tracing::warn!(target: "ozmux_gui::commands", ?err, "split_pane failed");
            false
        }
        None => {
            tracing::warn!(target: "ozmux_gui::commands", ?session_id, "Split: session vanished");
            false
        }
    }
}

fn apply_new_activity(mux: &mut MultiplexerService, session: &SessionId) -> bool {
    let session_id = *session;
    let active_pane = match mux.sessions.get(&session_id) {
        Some(s) => s.active_pane.clone(),
        None => {
            tracing::warn!(target: "ozmux_gui::commands", ?session_id, "NewActivity: session vanished");
            return false;
        }
    };
    let new_id = ActivityId::new();
    let activity = Activity::terminal(new_id.clone());
    let outcome = mux.with_session(&session_id, |s| -> MultiplexerResult<()> {
        let pane = s.pane_mut(&active_pane)?;
        pane.add_activity(activity)?;
        let _ = pane.set_active_activity(&new_id)?;
        Ok(())
    });
    match outcome {
        Some(Ok(())) => true,
        Some(Err(err)) => {
            tracing::warn!(target: "ozmux_gui::commands", ?err, "NewActivity failed");
            false
        }
        None => {
            tracing::warn!(target: "ozmux_gui::commands", ?session_id, "NewActivity: session vanished");
            false
        }
    }
}

fn apply_focus_pane(
    mux: &mut MultiplexerService,
    session: &SessionId,
    direction: PaneDirection,
) -> bool {
    let session_id = *session;
    let current = match mux.sessions.get(&session_id) {
        Some(s) => s.active_pane.clone(),
        None => {
            tracing::warn!(target: "ozmux_gui::commands", ?session_id, "FocusPane: session vanished");
            return false;
        }
    };
    let outcome = mux.with_session(&session_id, |s| -> MultiplexerResult<bool> {
        let Some(target) = s.pane_in_direction(&current, direction)? else {
            return Ok(false);
        };
        // NOTE: Treat "already active" as a no-op so change detection is
        // only tripped on real focus moves.
        Ok(matches!(
            s.set_active_pane(&target)?,
            SetActiveOutcome::Changed
        ))
    });
    match outcome {
        Some(Ok(mutated)) => mutated,
        Some(Err(err)) => {
            tracing::warn!(target: "ozmux_gui::commands", ?err, "FocusPane failed");
            false
        }
        None => {
            tracing::warn!(target: "ozmux_gui::commands", ?session_id, "FocusPane: session vanished");
            false
        }
    }
}

/// Focus the pane with the given id within the given session.
/// Returns `true` when `Session::active_pane` actually changed.
///
/// Caller is responsible for the change-detection discipline used by
/// other Multiplexer mutators (see `src/input.rs:330-336`):
///
/// ```ignore
/// let mux_ref = mux.bypass_change_detection();
/// let mutated = focus_pane_by_id(mux_ref, &session_id, &pane_id);
/// if mutated {
///     mux.bump_epoch(&session_id);
///     mux.set_changed();
/// }
/// ```
///
/// This helper is separate from `apply_focus_pane` (direction-based);
/// it serves the click-to-focus path in `src/input/mouse_buttons.rs`
/// where the target pane id is known directly.
pub(crate) fn focus_pane_by_id(
    mux: &mut MultiplexerService,
    session: &SessionId,
    target: &PaneId,
) -> bool {
    let session_id = *session;
    let outcome = mux.with_session(&session_id, |s| -> MultiplexerResult<bool> {
        // NOTE: Treat "already active" / unknown pane as a no-op so
        // change detection is only tripped on real focus moves.
        if &s.active_pane == target {
            return Ok(false);
        }
        Ok(matches!(
            s.set_active_pane(target)?,
            SetActiveOutcome::Changed
        ))
    });
    match outcome {
        Some(Ok(mutated)) => mutated,
        Some(Err(err)) => {
            tracing::warn!(target: "ozmux_gui::commands", ?err, "focus_pane_by_id failed");
            false
        }
        None => {
            tracing::warn!(target: "ozmux_gui::commands", ?session_id, "focus_pane_by_id: session vanished");
            false
        }
    }
}

fn apply_focus_activity(
    mux: &mut MultiplexerService,
    session: &SessionId,
    direction: CycleDirection,
) -> bool {
    let session_id = *session;
    let pid = match mux.sessions.get(&session_id) {
        Some(s) => s.active_pane.clone(),
        None => {
            tracing::warn!(target: "ozmux_gui::commands", ?session_id, "FocusActivity: session vanished");
            return false;
        }
    };
    let outcome = mux.with_session(&session_id, |s| -> MultiplexerResult<bool> {
        let pane = s.pane_mut(&pid)?;
        Ok(matches!(
            pane.cycle_active_activity(direction)?,
            SetActiveOutcome::Changed
        ))
    });
    match outcome {
        Some(Ok(mutated)) => mutated,
        Some(Err(err)) => {
            tracing::warn!(target: "ozmux_gui::commands", ?err, "FocusActivity failed");
            false
        }
        None => {
            tracing::warn!(target: "ozmux_gui::commands", ?session_id, "FocusActivity: session vanished");
            false
        }
    }
}

fn apply_swap_pane(mux: &mut MultiplexerService, session: &SessionId, offset: SwapOffset) -> bool {
    let session_id = *session;
    let current = match mux.sessions.get(&session_id) {
        Some(s) => s.active_pane.clone(),
        None => {
            tracing::warn!(target: "ozmux_gui::commands", ?session_id, "SwapPane: session vanished");
            return false;
        }
    };
    let outcome = mux.with_session(&session_id, |s| s.swap_pane(&current, offset));
    match outcome {
        Some(Ok(SwapOutcome::Swapped { .. })) => true,
        Some(Ok(SwapOutcome::NoOp)) => false,
        Some(Err(err)) => {
            tracing::warn!(target: "ozmux_gui::commands", ?err, "SwapPane failed");
            false
        }
        None => {
            tracing::warn!(target: "ozmux_gui::commands", ?session_id, "SwapPane: session vanished");
            false
        }
    }
}

fn apply_close_pane(mux: &mut MultiplexerService, session: &SessionId) -> bool {
    let session_id = *session;
    let pid = match mux.sessions.get(&session_id) {
        Some(s) => s.active_pane.clone(),
        None => {
            tracing::warn!(target: "ozmux_gui::commands", ?session_id, "ClosePane: session vanished");
            return false;
        }
    };
    let outcome = mux.with_session(&session_id, |s| s.close_pane(&pid));
    match outcome {
        Some(Ok(_destroyed)) => {
            mux.pane_owner_session.remove(&pid);
            true
        }
        Some(Err(err)) => {
            tracing::warn!(target: "ozmux_gui::commands", ?err, "ClosePane failed");
            false
        }
        None => {
            tracing::warn!(target: "ozmux_gui::commands", ?session_id, "ClosePane: session vanished");
            false
        }
    }
}

fn apply_close_activity(mux: &mut MultiplexerService, session: &SessionId) -> bool {
    let session_id = *session;
    let pid = match mux.sessions.get(&session_id) {
        Some(s) => s.active_pane.clone(),
        None => {
            tracing::warn!(target: "ozmux_gui::commands", ?session_id, "CloseActivity: session vanished");
            return false;
        }
    };
    let outcome = mux.with_session(&session_id, |s| -> MultiplexerResult<()> {
        let pane = s.pane_mut(&pid)?;
        let aid = pane.active_activity.clone();
        pane.remove_activity(&aid)?;
        Ok(())
    });
    match outcome {
        Some(Ok(())) => true,
        Some(Err(err)) => {
            tracing::warn!(target: "ozmux_gui::commands", ?err, "CloseActivity failed");
            false
        }
        None => {
            tracing::warn!(target: "ozmux_gui::commands", ?session_id, "CloseActivity: session vanished");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ozmux_configs::shortcuts::{Action, ActivityOffset, Direction, SplitDirection, SwapOffset};
    use ozmux_multiplexer::{
        Activity as MxActivity, ActivityId as MxActivityId, MultiplexerService,
    };

    fn fresh() -> (MultiplexerService, SessionId) {
        let mut svc = MultiplexerService::default();
        let (sid, _, _) = svc.create_session(Some("test".into()));
        (svc, sid)
    }

    #[test]
    fn new_session_action_short_circuits_in_apply() {
        let (mut svc, sid) = fresh();
        let mutated = apply(Action::NewSession, &mut svc, sid);
        assert!(!mutated);
        assert_eq!(svc.sessions.len(), 1);
    }

    #[test]
    fn focus_session_action_short_circuits_in_apply() {
        use ozmux_configs::shortcuts::SessionOffset;
        let (mut svc, sid) = fresh();
        let mutated = apply(
            Action::FocusSession {
                offset: SessionOffset::Next,
            },
            &mut svc,
            sid,
        );
        assert!(!mutated);
    }

    #[test]
    fn focus_session_number_action_short_circuits_in_apply() {
        let (mut svc, sid) = fresh();
        let mutated = apply(Action::FocusSessionNumber { index: 0 }, &mut svc, sid);
        assert!(!mutated);
    }

    #[test]
    fn split_pane_horizontal_action_adds_pane_to_session() {
        let (mut svc, sid) = fresh();
        let before = svc.sessions.get(&sid).unwrap().pane_ids().count();
        let mutated = apply(
            Action::SplitPane {
                direction: SplitDirection::Horizontal,
            },
            &mut svc,
            sid,
        );
        assert!(mutated);
        let after = svc.sessions.get(&sid).unwrap().pane_ids().count();
        assert_eq!(after, before + 1);
    }

    #[test]
    fn split_pane_vertical_action_adds_pane_to_session() {
        let (mut svc, sid) = fresh();
        let before = svc.sessions.get(&sid).unwrap().pane_ids().count();
        let mutated = apply(
            Action::SplitPane {
                direction: SplitDirection::Vertical,
            },
            &mut svc,
            sid,
        );
        assert!(mutated);
        let after = svc.sessions.get(&sid).unwrap().pane_ids().count();
        assert_eq!(after, before + 1);
    }

    #[test]
    fn split_pane_registers_pane_owner_session_entry() {
        let (mut svc, sid) = fresh();
        let before = svc.pane_owner_session.len();
        apply(
            Action::SplitPane {
                direction: SplitDirection::Horizontal,
            },
            &mut svc,
            sid,
        );
        assert_eq!(svc.pane_owner_session.len(), before + 1);
    }

    #[test]
    fn split_pane_promotes_new_pane_to_active() {
        let (mut svc, sid) = fresh();
        let original_active = svc.sessions.get(&sid).unwrap().active_pane.clone();
        apply(
            Action::SplitPane {
                direction: SplitDirection::Horizontal,
            },
            &mut svc,
            sid,
        );
        let new_active = svc.sessions.get(&sid).unwrap().active_pane.clone();
        assert_ne!(new_active, original_active);
    }

    #[test]
    fn new_terminal_activity_adds_and_activates_activity_on_active_pane() {
        let (mut svc, sid) = fresh();
        let pid = svc.sessions.get(&sid).unwrap().active_pane.clone();
        let before = svc
            .sessions
            .get(&sid)
            .unwrap()
            .pane(&pid)
            .unwrap()
            .activity_ids()
            .count();
        let mutated = apply(Action::NewTerminalActivity, &mut svc, sid);
        assert!(mutated);
        let session = svc.sessions.get(&sid).unwrap();
        let pane = session.pane(&pid).unwrap();
        assert_eq!(pane.activity_ids().count(), before + 1);
        let active_id = pane.active_activity.clone();
        assert!(pane.has_activity(&active_id));
    }

    #[test]
    fn unimplemented_action_returns_false_without_state_change() {
        let (mut svc, sid) = fresh();
        let panes_before = svc.sessions.get(&sid).unwrap().pane_ids().count();
        let mutated = apply(Action::ZoomPane, &mut svc, sid);
        assert!(!mutated, "unimplemented variant must return false");
        assert_eq!(
            svc.sessions.get(&sid).unwrap().pane_ids().count(),
            panes_before,
        );
    }

    #[test]
    fn focus_pane_in_single_pane_session_returns_false() {
        let (mut svc, sid) = fresh();
        let mutated = apply(
            Action::FocusPane {
                direction: Direction::Right,
            },
            &mut svc,
            sid,
        );
        assert!(!mutated);
    }

    #[test]
    fn focus_pane_left_moves_active_to_left_neighbor() {
        let (mut svc, sid) = fresh();
        apply(
            Action::SplitPane {
                direction: SplitDirection::Horizontal,
            },
            &mut svc,
            sid,
        );
        let right_pane = svc.sessions.get(&sid).unwrap().active_pane.clone();
        let mutated = apply(
            Action::FocusPane {
                direction: Direction::Left,
            },
            &mut svc,
            sid,
        );
        assert!(mutated);
        let new_active = svc.sessions.get(&sid).unwrap().active_pane.clone();
        assert_ne!(new_active, right_pane);
    }

    #[test]
    fn swap_pane_in_single_pane_session_returns_false() {
        let (mut svc, sid) = fresh();
        let mutated = apply(
            Action::SwapPane {
                offset: SwapOffset::Prev,
            },
            &mut svc,
            sid,
        );
        assert!(!mutated);
    }

    #[test]
    fn close_pane_action_removes_pane_and_promotes_survivor() {
        let (mut svc, sid) = fresh();
        apply(
            Action::SplitPane {
                direction: SplitDirection::Horizontal,
            },
            &mut svc,
            sid,
        );
        let target = svc.sessions.get(&sid).unwrap().active_pane.clone();
        let before = svc.sessions.get(&sid).unwrap().pane_ids().count();
        let mutated = apply(Action::ClosePane, &mut svc, sid);
        assert!(mutated);
        let session = svc.sessions.get(&sid).unwrap();
        assert_eq!(session.pane_ids().count(), before - 1);
        assert_ne!(session.active_pane, target);
    }

    #[test]
    fn close_pane_removes_pane_owner_session_entry() {
        let (mut svc, sid) = fresh();
        apply(
            Action::SplitPane {
                direction: SplitDirection::Horizontal,
            },
            &mut svc,
            sid,
        );
        let closed_pane = svc.sessions.get(&sid).unwrap().active_pane.clone();
        assert!(svc.pane_owner_session.contains_key(&closed_pane));
        apply(Action::ClosePane, &mut svc, sid);
        assert!(!svc.pane_owner_session.contains_key(&closed_pane));
    }

    #[test]
    fn close_pane_in_single_pane_session_returns_false() {
        let (mut svc, sid) = fresh();
        let mutated = apply(Action::ClosePane, &mut svc, sid);
        assert!(!mutated);
    }

    #[test]
    fn close_activity_action_removes_active_activity() {
        let (mut svc, sid) = fresh();
        let pid = svc.sessions.get(&sid).unwrap().active_pane.clone();

        let appended_id = MxActivityId::new();
        let appended = MxActivity::terminal(appended_id.clone());
        svc.with_session(&sid, |s| -> MultiplexerResult<()> {
            let pane = s.pane_mut(&pid)?;
            pane.add_activity(appended)?;
            let _ = pane.set_active_activity(&appended_id)?;
            Ok(())
        })
        .expect("session exists")
        .expect("add_activity + set_active_activity succeeded");

        let mutated = apply(Action::CloseActivity, &mut svc, sid);
        assert!(mutated);
        let pane = svc.sessions.get(&sid).unwrap().pane(&pid).unwrap();
        assert!(!pane.has_activity(&appended_id));
    }

    #[test]
    fn close_activity_in_single_activity_pane_returns_false() {
        let (mut svc, sid) = fresh();
        let mutated = apply(Action::CloseActivity, &mut svc, sid);
        assert!(!mutated);
    }

    #[test]
    fn focus_activity_next_advances_active_activity() {
        let (mut svc, sid) = fresh();
        let pid = svc.sessions.get(&sid).unwrap().active_pane.clone();

        let appended_id = MxActivityId::new();
        let appended = MxActivity::terminal(appended_id.clone());
        svc.with_session(&sid, |s| -> MultiplexerResult<()> {
            s.pane_mut(&pid)?.add_activity(appended)?;
            Ok(())
        })
        .expect("session exists")
        .expect("add_activity succeeded");

        let active_before = svc
            .sessions
            .get(&sid)
            .unwrap()
            .pane(&pid)
            .unwrap()
            .active_activity
            .clone();
        let mutated = apply(
            Action::FocusActivity {
                offset: ActivityOffset::Next,
            },
            &mut svc,
            sid,
        );
        assert!(mutated);
        let active_after = svc
            .sessions
            .get(&sid)
            .unwrap()
            .pane(&pid)
            .unwrap()
            .active_activity
            .clone();
        assert_ne!(active_after, active_before);
        assert_eq!(active_after, appended_id);
    }

    #[test]
    fn focus_activity_in_single_activity_pane_returns_false() {
        let (mut svc, sid) = fresh();
        let mutated = apply(
            Action::FocusActivity {
                offset: ActivityOffset::Next,
            },
            &mut svc,
            sid,
        );
        assert!(!mutated);
    }

    #[test]
    fn focus_activity_last_returns_false_without_state_change() {
        let (mut svc, sid) = fresh();
        let mutated = apply(
            Action::FocusActivity {
                offset: ActivityOffset::Last,
            },
            &mut svc,
            sid,
        );
        assert!(!mutated);
    }

    #[test]
    fn focus_pane_by_id_switches_active_pane() {
        let (mut svc, sid) = fresh();
        let original_active = svc.sessions.get(&sid).unwrap().active_pane.clone();
        apply(
            Action::SplitPane {
                direction: SplitDirection::Horizontal,
            },
            &mut svc,
            sid,
        );
        let new_active = svc.sessions.get(&sid).unwrap().active_pane.clone();
        assert_ne!(new_active, original_active, "split should promote new pane");

        let mutated = focus_pane_by_id(&mut svc, &sid, &original_active);
        assert!(mutated, "focusing a different pane must return true");
        assert_eq!(
            svc.sessions.get(&sid).unwrap().active_pane,
            original_active,
            "active pane should be the target"
        );
    }

    #[test]
    fn focus_pane_by_id_returns_false_when_already_active() {
        let (mut svc, sid) = fresh();
        let already_active = svc.sessions.get(&sid).unwrap().active_pane.clone();

        let mutated = focus_pane_by_id(&mut svc, &sid, &already_active);
        assert!(
            !mutated,
            "no-op focus on already-active pane must return false"
        );
        assert_eq!(
            svc.sessions.get(&sid).unwrap().active_pane,
            already_active,
            "active pane should be unchanged"
        );
    }

    #[test]
    fn focus_pane_by_id_returns_false_for_unknown_pane() {
        let (mut svc, sid) = fresh();
        let bogus = PaneId::new();

        let mutated = focus_pane_by_id(&mut svc, &sid, &bogus);
        assert!(
            !mutated,
            "unknown pane must not change focus and return false"
        );
    }
}
