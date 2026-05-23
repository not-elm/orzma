//! Pure (Bevy-independent) functions that apply a `configs::Action` to the
//! domain `MultiplexerService`. Called by the shortcut dispatcher.

use ozmux_configs::shortcuts::{
    Action, ActivityOffset as ConfigActivityOffset, Direction as ConfigDirection, SplitDirection,
    SwapOffset as ConfigSwapOffset, WindowOffset,
};
use ozmux_multiplexer::{
    Activity, ActivityId, CycleDirection, MultiplexerResult, MultiplexerService, PaneDirection,
    PaneId, SessionId, SetActiveOutcome, Side, SplitOrientation, SwapOffset, SwapOutcome,
};

/// Applies `action` to the domain `MultiplexerService` for the given session.
/// Returns `true` if the domain state was mutated (caller may then trip
/// Bevy change detection); `false` if the action could not be applied,
/// short-circuited on validation, or is not implemented yet in this branch.
/// Bevy-independent so it can be unit-tested without an `App`.
pub fn apply(action: Action, mux: &mut MultiplexerService, session: SessionId) -> bool {
    match action {
        Action::NewWindow => apply_new_window(mux, &session),
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
        Action::FocusWindow { offset } => match window_cycle_direction(offset) {
            Some(direction) => apply_focus_window(mux, &session, direction),
            None => {
                tracing::debug!(
                    target: "ozmux_gui::commands",
                    "FocusWindow::Last not yet implemented"
                );
                false
            }
        },
        Action::ClosePane => apply_close_pane(mux, &session),
        Action::CloseActivity => apply_close_activity(mux, &session),
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

fn apply_new_window(mux: &mut MultiplexerService, session: &SessionId) -> bool {
    match mux.create_window(Some(session), None) {
        Ok(_) => true,
        Err(err) => {
            tracing::warn!(target: "ozmux_gui::commands", ?err, "NewWindow failed");
            false
        }
    }
}

fn apply_split(
    mux: &mut MultiplexerService,
    session: &SessionId,
    orientation: SplitOrientation,
) -> bool {
    let (active_window, active_pane) = match mux.active_pane_of_session(session) {
        Ok(target) => target,
        Err(err) => {
            tracing::warn!(target: "ozmux_gui::commands", ?err, "Split: resolve active pane failed");
            return false;
        }
    };

    let new_pane_id = PaneId::new();
    let new_activity = Activity::terminal(ActivityId::new());
    let split = mux.with_window(&active_window, |w| {
        w.split_pane(
            &active_pane,
            new_pane_id.clone(),
            new_activity,
            Side::After,
            orientation,
        )
    });
    match split {
        Some(Ok(_)) => {
            mux.pane_owner_window.insert(new_pane_id, active_window);
            true
        }
        Some(Err(err)) => {
            tracing::warn!(target: "ozmux_gui::commands", ?err, "split_pane failed");
            false
        }
        None => {
            tracing::warn!(target: "ozmux_gui::commands", ?active_window, "Split: window vanished");
            false
        }
    }
}

fn apply_new_activity(mux: &mut MultiplexerService, session: &SessionId) -> bool {
    let (active_window, active_pane) = match mux.active_pane_of_session(session) {
        Ok(target) => target,
        Err(err) => {
            tracing::warn!(target: "ozmux_gui::commands", ?err, "NewActivity: resolve active pane failed");
            return false;
        }
    };

    let new_id = ActivityId::new();
    let activity = Activity::terminal(new_id.clone());
    let outcome = mux.with_window(&active_window, |w| -> MultiplexerResult<()> {
        let pane = w.pane_mut(&active_pane)?;
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
            tracing::warn!(target: "ozmux_gui::commands", ?active_window, "NewActivity: window vanished");
            false
        }
    }
}

fn apply_focus_pane(
    mux: &mut MultiplexerService,
    session: &SessionId,
    direction: PaneDirection,
) -> bool {
    let (wid, current) = match mux.active_pane_of_session(session) {
        Ok(target) => target,
        Err(err) => {
            tracing::warn!(target: "ozmux_gui::commands", ?err, "FocusPane: resolve active pane failed");
            return false;
        }
    };
    let outcome = mux.with_window(&wid, |w| -> MultiplexerResult<bool> {
        let Some(target) = w.pane_in_direction(&current, direction)? else {
            return Ok(false);
        };
        // NOTE: Treat "already active" as a no-op so change detection is
        // only tripped on real focus moves.
        Ok(matches!(
            w.set_active_pane(&target)?,
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
            tracing::warn!(target: "ozmux_gui::commands", ?wid, "FocusPane: window vanished");
            false
        }
    }
}

fn apply_focus_activity(
    mux: &mut MultiplexerService,
    session: &SessionId,
    direction: CycleDirection,
) -> bool {
    let (wid, pid) = match mux.active_pane_of_session(session) {
        Ok(target) => target,
        Err(err) => {
            tracing::warn!(target: "ozmux_gui::commands", ?err, "FocusActivity: resolve active pane failed");
            return false;
        }
    };
    let outcome = mux.with_window(&wid, |w| -> MultiplexerResult<bool> {
        let pane = w.pane_mut(&pid)?;
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
            tracing::warn!(target: "ozmux_gui::commands", ?wid, "FocusActivity: window vanished");
            false
        }
    }
}

fn apply_swap_pane(mux: &mut MultiplexerService, session: &SessionId, offset: SwapOffset) -> bool {
    let (wid, current) = match mux.active_pane_of_session(session) {
        Ok(target) => target,
        Err(err) => {
            tracing::warn!(target: "ozmux_gui::commands", ?err, "SwapPane: resolve active pane failed");
            return false;
        }
    };
    let outcome = mux.with_window(&wid, |w| w.swap_pane(&current, offset));
    match outcome {
        Some(Ok(SwapOutcome::Swapped { .. })) => true,
        Some(Ok(SwapOutcome::NoOp)) => false,
        Some(Err(err)) => {
            tracing::warn!(target: "ozmux_gui::commands", ?err, "SwapPane failed");
            false
        }
        None => {
            tracing::warn!(target: "ozmux_gui::commands", ?wid, "SwapPane: window vanished");
            false
        }
    }
}

fn apply_close_pane(mux: &mut MultiplexerService, session: &SessionId) -> bool {
    let (wid, pid) = match mux.active_pane_of_session(session) {
        Ok(target) => target,
        Err(err) => {
            tracing::warn!(target: "ozmux_gui::commands", ?err, "ClosePane: resolve active pane failed");
            return false;
        }
    };
    let outcome = mux.with_window(&wid, |w| w.close_pane(&pid));
    match outcome {
        Some(Ok(_destroyed)) => {
            mux.pane_owner_window.remove(&pid);
            true
        }
        Some(Err(err)) => {
            tracing::warn!(target: "ozmux_gui::commands", ?err, "ClosePane failed");
            false
        }
        None => {
            tracing::warn!(target: "ozmux_gui::commands", ?wid, "ClosePane: window vanished");
            false
        }
    }
}

fn apply_close_activity(mux: &mut MultiplexerService, session: &SessionId) -> bool {
    let (wid, pid) = match mux.active_pane_of_session(session) {
        Ok(target) => target,
        Err(err) => {
            tracing::warn!(target: "ozmux_gui::commands", ?err, "CloseActivity: resolve active pane failed");
            return false;
        }
    };
    let outcome = mux.with_window(&wid, |w| -> MultiplexerResult<()> {
        let pane = w.pane_mut(&pid)?;
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
            tracing::warn!(target: "ozmux_gui::commands", ?wid, "CloseActivity: window vanished");
            false
        }
    }
}

fn window_cycle_direction(o: WindowOffset) -> Option<CycleDirection> {
    match o {
        WindowOffset::Next => Some(CycleDirection::Next),
        WindowOffset::Prev => Some(CycleDirection::Prev),
        WindowOffset::Last => None,
    }
}

fn apply_focus_window(
    mux: &mut MultiplexerService,
    session: &SessionId,
    direction: CycleDirection,
) -> bool {
    match mux.cycle_active_window(session, direction) {
        Ok(SetActiveOutcome::Changed) => true,
        Ok(SetActiveOutcome::Unchanged) => false,
        Err(err) => {
            tracing::warn!(target: "ozmux_gui::commands", ?err, "FocusWindow failed");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_window_action_creates_window_attached_to_session() {
        let mut svc = MultiplexerService::default();
        let sid = svc.create_session(Some("default".into()));
        let windows_before = svc.windows.len();

        apply(Action::NewWindow, &mut svc, sid.clone());

        assert_eq!(svc.windows.len(), windows_before + 1);
        let session = svc.sessions.get(&sid).expect("session still exists");
        assert_eq!(session.linked_windows.len(), 1);
    }

    #[test]
    fn split_pane_horizontal_action_adds_pane_to_active_window() {
        let mut svc = MultiplexerService::default();
        let sid = svc.create_session(Some("default".into()));
        apply(Action::NewWindow, &mut svc, sid.clone());

        let wid = svc.sessions.get(&sid).unwrap().linked_windows[0].clone();
        let panes_before = svc.windows.get(&wid).unwrap().pane_ids().count();
        let original_pane = svc.windows.get(&wid).unwrap().active_pane.clone();

        apply(
            Action::SplitPane {
                direction: SplitDirection::Horizontal,
            },
            &mut svc,
            sid.clone(),
        );

        let window = svc.windows.get(&wid).unwrap();
        assert_eq!(window.pane_ids().count(), panes_before + 1);
        assert_ne!(
            window.active_pane, original_pane,
            "active_pane must promote to new pane"
        );
    }

    #[test]
    fn split_pane_vertical_action_adds_pane_to_active_window() {
        let mut svc = MultiplexerService::default();
        let sid = svc.create_session(Some("default".into()));
        apply(Action::NewWindow, &mut svc, sid.clone());

        let wid = svc.sessions.get(&sid).unwrap().linked_windows[0].clone();
        let panes_before = svc.windows.get(&wid).unwrap().pane_ids().count();
        let original_pane = svc.windows.get(&wid).unwrap().active_pane.clone();

        apply(
            Action::SplitPane {
                direction: SplitDirection::Vertical,
            },
            &mut svc,
            sid,
        );

        let window = svc.windows.get(&wid).unwrap();
        assert_eq!(window.pane_ids().count(), panes_before + 1);
        assert_ne!(
            window.active_pane, original_pane,
            "active_pane must promote to new pane"
        );
    }

    #[test]
    fn new_terminal_activity_adds_and_activates_activity_on_active_pane() {
        let mut svc = MultiplexerService::default();
        let sid = svc.create_session(Some("default".into()));
        apply(Action::NewWindow, &mut svc, sid.clone());

        let wid = svc.sessions.get(&sid).unwrap().linked_windows[0].clone();
        let pid = svc.windows.get(&wid).unwrap().active_pane.clone();

        let activities_before = svc
            .windows
            .get(&wid)
            .unwrap()
            .pane(&pid)
            .unwrap()
            .activity_ids()
            .count();

        apply(Action::NewTerminalActivity, &mut svc, sid);

        let pane = svc.windows.get(&wid).unwrap().pane(&pid).unwrap();
        assert_eq!(pane.activity_ids().count(), activities_before + 1);
        let new_active = pane.active_activity.clone();
        assert!(pane.has_activity(&new_active));
    }

    #[test]
    fn unimplemented_action_returns_false_without_state_change() {
        let mut svc = MultiplexerService::default();
        let sid = svc.create_session(Some("default".into()));
        apply(Action::NewWindow, &mut svc, sid.clone());

        let windows_before = svc.windows.len();

        let mutated = apply(Action::ZoomPane, &mut svc, sid);

        assert!(!mutated, "unimplemented variant must return false");
        assert_eq!(svc.windows.len(), windows_before);
    }

    #[test]
    fn focus_pane_left_moves_active_to_left_neighbor() {
        use ozmux_configs::shortcuts::Direction;

        let mut svc = MultiplexerService::default();
        let sid = svc.create_session(Some("default".into()));
        apply(Action::NewWindow, &mut svc, sid.clone());

        // After SplitPane{Horizontal}, the new (right-side) pane becomes active.
        apply(
            Action::SplitPane {
                direction: SplitDirection::Horizontal,
            },
            &mut svc,
            sid.clone(),
        );

        let wid = svc.sessions.get(&sid).unwrap().linked_windows[0].clone();
        let right_pane = svc.windows.get(&wid).unwrap().active_pane.clone();

        let mutated = apply(
            Action::FocusPane {
                direction: Direction::Left,
            },
            &mut svc,
            sid,
        );

        assert!(mutated, "FocusPane Left from right pane must mutate");
        let new_active = svc.windows.get(&wid).unwrap().active_pane.clone();
        assert_ne!(new_active, right_pane, "active_pane must change");
    }

    #[test]
    fn focus_pane_with_no_neighbor_returns_false_and_keeps_active() {
        use ozmux_configs::shortcuts::Direction;

        let mut svc = MultiplexerService::default();
        let sid = svc.create_session(Some("default".into()));
        apply(Action::NewWindow, &mut svc, sid.clone());

        let wid = svc.sessions.get(&sid).unwrap().linked_windows[0].clone();
        let original_active = svc.windows.get(&wid).unwrap().active_pane.clone();

        // Single-pane window — no neighbor in any direction.
        let mutated = apply(
            Action::FocusPane {
                direction: Direction::Up,
            },
            &mut svc,
            sid,
        );

        assert!(!mutated, "single-pane window: FocusPane must return false");
        assert_eq!(
            svc.windows.get(&wid).unwrap().active_pane,
            original_active,
            "active_pane must not change"
        );
    }

    #[test]
    fn swap_pane_next_swaps_cells_keeps_active_pane_id() {
        use ozmux_configs::shortcuts::SwapOffset;
        use ozmux_multiplexer::PaneDirection;

        let mut svc = MultiplexerService::default();
        let sid = svc.create_session(Some("default".into()));
        apply(Action::NewWindow, &mut svc, sid.clone());
        apply(
            Action::SplitPane {
                direction: SplitDirection::Horizontal,
            },
            &mut svc,
            sid.clone(),
        );

        let wid = svc.sessions.get(&sid).unwrap().linked_windows[0].clone();
        let active_before = svc.windows.get(&wid).unwrap().active_pane.clone();
        // Capture the other pane id for the post-swap assertion.
        let other_before = svc
            .windows
            .get(&wid)
            .unwrap()
            .pane_in_direction(&active_before, PaneDirection::Left)
            .unwrap()
            .expect("left neighbor must exist after horizontal split");

        let mutated = apply(
            Action::SwapPane {
                offset: SwapOffset::Next,
            },
            &mut svc,
            sid,
        );

        assert!(mutated, "SwapPane on 2-pane window must mutate");
        let active_after = svc.windows.get(&wid).unwrap().active_pane.clone();
        assert_eq!(
            active_after, active_before,
            "active_pane PaneId is unchanged — only the cell moves"
        );

        // After Next-swap the active pane sits to the geometric left, so the
        // other pane is now its Right neighbor (no wrap-around needed).
        let right_neighbor = svc
            .windows
            .get(&wid)
            .unwrap()
            .pane_in_direction(&active_after, PaneDirection::Right)
            .unwrap()
            .expect("right neighbor must exist after swap");
        assert_eq!(
            right_neighbor, other_before,
            "the other pane is now to the right of the (still-same-id) active pane"
        );
    }

    #[test]
    fn swap_pane_in_single_pane_window_returns_false() {
        use ozmux_configs::shortcuts::SwapOffset;

        let mut svc = MultiplexerService::default();
        let sid = svc.create_session(Some("default".into()));
        apply(Action::NewWindow, &mut svc, sid.clone());

        let wid = svc.sessions.get(&sid).unwrap().linked_windows[0].clone();
        let panes_before = svc.windows.get(&wid).unwrap().pane_ids().count();

        let mutated = apply(
            Action::SwapPane {
                offset: SwapOffset::Prev,
            },
            &mut svc,
            sid,
        );

        assert!(!mutated, "single-pane window: SwapPane must return false");
        assert_eq!(
            svc.windows.get(&wid).unwrap().pane_ids().count(),
            panes_before,
            "pane count must not change"
        );
    }

    #[test]
    fn close_pane_action_removes_pane_and_promotes_survivor() {
        let mut svc = MultiplexerService::default();
        let sid = svc.create_session(Some("default".into()));
        apply(Action::NewWindow, &mut svc, sid.clone());
        apply(
            Action::SplitPane {
                direction: SplitDirection::Horizontal,
            },
            &mut svc,
            sid.clone(),
        );

        let wid = svc.sessions.get(&sid).unwrap().linked_windows[0].clone();
        let panes_before = svc.windows.get(&wid).unwrap().pane_ids().count();
        let target_pane = svc.windows.get(&wid).unwrap().active_pane.clone();

        let mutated = apply(Action::ClosePane, &mut svc, sid);

        assert!(mutated, "ClosePane on a 2-pane window must mutate");
        let window = svc.windows.get(&wid).unwrap();
        assert_eq!(window.pane_ids().count(), panes_before - 1);
        assert_ne!(
            window.active_pane, target_pane,
            "active_pane must promote to the surviving pane"
        );
    }

    #[test]
    fn close_pane_action_removes_pane_owner_window_entry() {
        let mut svc = MultiplexerService::default();
        let sid = svc.create_session(Some("default".into()));
        apply(Action::NewWindow, &mut svc, sid.clone());
        apply(
            Action::SplitPane {
                direction: SplitDirection::Horizontal,
            },
            &mut svc,
            sid.clone(),
        );

        let wid = svc.sessions.get(&sid).unwrap().linked_windows[0].clone();
        let closed_pane = svc.windows.get(&wid).unwrap().active_pane.clone();
        assert!(
            svc.pane_owner_window.contains_key(&closed_pane),
            "split must register the new pane in pane_owner_window"
        );

        let mutated = apply(Action::ClosePane, &mut svc, sid);

        assert!(mutated);
        assert!(
            !svc.pane_owner_window.contains_key(&closed_pane),
            "close_pane must remove the closed pane from pane_owner_window"
        );
    }

    #[test]
    fn close_pane_in_single_pane_window_returns_false() {
        let mut svc = MultiplexerService::default();
        let sid = svc.create_session(Some("default".into()));
        apply(Action::NewWindow, &mut svc, sid.clone());

        let wid = svc.sessions.get(&sid).unwrap().linked_windows[0].clone();
        let panes_before = svc.windows.get(&wid).unwrap().pane_ids().count();
        let active_before = svc.windows.get(&wid).unwrap().active_pane.clone();

        let mutated = apply(Action::ClosePane, &mut svc, sid);

        assert!(
            !mutated,
            "single-pane window: ClosePane must return false (warn-only)"
        );
        let window = svc.windows.get(&wid).unwrap();
        assert_eq!(window.pane_ids().count(), panes_before);
        assert_eq!(window.active_pane, active_before);
    }

    #[test]
    fn close_activity_action_removes_active_activity() {
        use ozmux_multiplexer::Activity as MxActivity;
        use ozmux_multiplexer::ActivityId as MxActivityId;

        let mut svc = MultiplexerService::default();
        let sid = svc.create_session(Some("default".into()));
        apply(Action::NewWindow, &mut svc, sid.clone());

        let wid = svc.sessions.get(&sid).unwrap().linked_windows[0].clone();
        let pid = svc.windows.get(&wid).unwrap().active_pane.clone();

        let appended_id = MxActivityId::new();
        let appended = MxActivity::terminal(appended_id.clone());
        svc.with_window(&wid, |w| -> MultiplexerResult<()> {
            let pane = w.pane_mut(&pid)?;
            pane.add_activity(appended)?;
            let _ = pane.set_active_activity(&appended_id)?;
            Ok(())
        })
        .expect("window exists")
        .expect("add_activity + set_active_activity succeeded");

        let activities_before = svc
            .windows
            .get(&wid)
            .unwrap()
            .pane(&pid)
            .unwrap()
            .activity_ids()
            .count();
        assert_eq!(activities_before, 2);

        let mutated = apply(Action::CloseActivity, &mut svc, sid);

        assert!(mutated, "CloseActivity on a 2-activity pane must mutate");
        let pane = svc.windows.get(&wid).unwrap().pane(&pid).unwrap();
        assert_eq!(pane.activity_ids().count(), activities_before - 1);
        assert!(
            !pane.has_activity(&appended_id),
            "the previously-active appended activity must be gone"
        );
        assert_ne!(
            pane.active_activity, appended_id,
            "active_activity must rebase to the remaining activity"
        );
    }

    #[test]
    fn close_activity_in_single_activity_pane_returns_false() {
        let mut svc = MultiplexerService::default();
        let sid = svc.create_session(Some("default".into()));
        apply(Action::NewWindow, &mut svc, sid.clone());

        let wid = svc.sessions.get(&sid).unwrap().linked_windows[0].clone();
        let pid = svc.windows.get(&wid).unwrap().active_pane.clone();
        let activities_before = svc
            .windows
            .get(&wid)
            .unwrap()
            .pane(&pid)
            .unwrap()
            .activity_ids()
            .count();
        let active_before = svc
            .windows
            .get(&wid)
            .unwrap()
            .pane(&pid)
            .unwrap()
            .active_activity
            .clone();
        assert_eq!(
            activities_before, 1,
            "fresh pane must have exactly 1 activity"
        );

        let mutated = apply(Action::CloseActivity, &mut svc, sid);

        assert!(
            !mutated,
            "single-activity pane: CloseActivity must return false (warn-only)"
        );
        let pane = svc.windows.get(&wid).unwrap().pane(&pid).unwrap();
        assert_eq!(pane.activity_ids().count(), activities_before);
        assert_eq!(pane.active_activity, active_before);
    }

    #[test]
    fn focus_activity_next_advances_active_activity() {
        use ozmux_configs::shortcuts::ActivityOffset;
        use ozmux_multiplexer::Activity as MxActivity;
        use ozmux_multiplexer::ActivityId as MxActivityId;

        let mut svc = MultiplexerService::default();
        let sid = svc.create_session(Some("default".into()));
        apply(Action::NewWindow, &mut svc, sid.clone());

        let wid = svc.sessions.get(&sid).unwrap().linked_windows[0].clone();
        let pid = svc.windows.get(&wid).unwrap().active_pane.clone();

        let appended_id = MxActivityId::new();
        let appended = MxActivity::terminal(appended_id.clone());
        svc.with_window(&wid, |w| -> MultiplexerResult<()> {
            w.pane_mut(&pid)?.add_activity(appended)?;
            Ok(())
        })
        .expect("window exists")
        .expect("add_activity succeeded");

        let active_before = svc
            .windows
            .get(&wid)
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

        assert!(
            mutated,
            "FocusActivity Next on a 2-activity pane must mutate"
        );
        let active_after = svc
            .windows
            .get(&wid)
            .unwrap()
            .pane(&pid)
            .unwrap()
            .active_activity
            .clone();
        assert_ne!(active_after, active_before, "active_activity must advance");
        assert_eq!(
            active_after, appended_id,
            "Next from index 0 must land on the appended (index 1) activity"
        );
    }

    #[test]
    fn focus_activity_prev_wraps_to_last() {
        use ozmux_configs::shortcuts::ActivityOffset;
        use ozmux_multiplexer::Activity as MxActivity;
        use ozmux_multiplexer::ActivityId as MxActivityId;

        let mut svc = MultiplexerService::default();
        let sid = svc.create_session(Some("default".into()));
        apply(Action::NewWindow, &mut svc, sid.clone());

        let wid = svc.sessions.get(&sid).unwrap().linked_windows[0].clone();
        let pid = svc.windows.get(&wid).unwrap().active_pane.clone();

        let appended_id = MxActivityId::new();
        let appended = MxActivity::terminal(appended_id.clone());
        svc.with_window(&wid, |w| -> MultiplexerResult<()> {
            w.pane_mut(&pid)?.add_activity(appended)?;
            Ok(())
        })
        .expect("window exists")
        .expect("add_activity succeeded");

        let mutated = apply(
            Action::FocusActivity {
                offset: ActivityOffset::Prev,
            },
            &mut svc,
            sid,
        );

        assert!(
            mutated,
            "FocusActivity Prev on a 2-activity pane must mutate"
        );
        let active_after = svc
            .windows
            .get(&wid)
            .unwrap()
            .pane(&pid)
            .unwrap()
            .active_activity
            .clone();
        assert_eq!(
            active_after, appended_id,
            "Prev from index 0 must wrap to the last (appended) activity"
        );
    }

    #[test]
    fn focus_activity_in_single_activity_pane_returns_false() {
        use ozmux_configs::shortcuts::ActivityOffset;

        let mut svc = MultiplexerService::default();
        let sid = svc.create_session(Some("default".into()));
        apply(Action::NewWindow, &mut svc, sid.clone());

        let wid = svc.sessions.get(&sid).unwrap().linked_windows[0].clone();
        let pid = svc.windows.get(&wid).unwrap().active_pane.clone();
        let active_before = svc
            .windows
            .get(&wid)
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

        assert!(
            !mutated,
            "single-activity pane: FocusActivity must return false (no change-detection trip)"
        );
        let active_after = svc
            .windows
            .get(&wid)
            .unwrap()
            .pane(&pid)
            .unwrap()
            .active_activity
            .clone();
        assert_eq!(
            active_after, active_before,
            "active_activity must not change"
        );
    }

    #[test]
    fn focus_activity_last_returns_false_without_state_change() {
        use ozmux_configs::shortcuts::ActivityOffset;

        let mut svc = MultiplexerService::default();
        let sid = svc.create_session(Some("default".into()));
        apply(Action::NewWindow, &mut svc, sid.clone());

        let wid = svc.sessions.get(&sid).unwrap().linked_windows[0].clone();
        let pid = svc.windows.get(&wid).unwrap().active_pane.clone();
        let active_before = svc
            .windows
            .get(&wid)
            .unwrap()
            .pane(&pid)
            .unwrap()
            .active_activity
            .clone();

        let mutated = apply(
            Action::FocusActivity {
                offset: ActivityOffset::Last,
            },
            &mut svc,
            sid,
        );

        assert!(
            !mutated,
            "FocusActivity::Last must return false (unimplemented)"
        );
        let active_after = svc
            .windows
            .get(&wid)
            .unwrap()
            .pane(&pid)
            .unwrap()
            .active_activity
            .clone();
        assert_eq!(
            active_after, active_before,
            "Last must not change active_activity"
        );
    }

    #[test]
    fn focus_window_next_advances_active_window() {
        use ozmux_configs::shortcuts::WindowOffset;

        let mut svc = MultiplexerService::default();
        let sid = svc.create_session(Some("default".into()));
        apply(Action::NewWindow, &mut svc, sid.clone());
        apply(Action::NewWindow, &mut svc, sid.clone());

        let active_before = svc
            .sessions
            .get(&sid)
            .unwrap()
            .active_window
            .clone()
            .expect("session must have an active window after NewWindow");
        assert_eq!(
            svc.sessions.get(&sid).unwrap().linked_windows.len(),
            2,
            "setup must produce exactly 2 windows"
        );

        let mutated = apply(
            Action::FocusWindow {
                offset: WindowOffset::Next,
            },
            &mut svc,
            sid.clone(),
        );

        assert!(mutated, "FocusWindow Next on 2-window session must mutate");
        let active_after = svc
            .sessions
            .get(&sid)
            .unwrap()
            .active_window
            .clone()
            .unwrap();
        assert_ne!(active_after, active_before, "active_window must advance");
    }

    #[test]
    fn focus_window_prev_wraps_to_last() {
        use ozmux_configs::shortcuts::WindowOffset;

        let mut svc = MultiplexerService::default();
        let sid = svc.create_session(Some("default".into()));
        apply(Action::NewWindow, &mut svc, sid.clone());
        apply(Action::NewWindow, &mut svc, sid.clone());

        let linked = svc.sessions.get(&sid).unwrap().linked_windows.clone();
        assert_eq!(linked.len(), 2);
        let w_last = linked[1].clone();

        let mutated = apply(
            Action::FocusWindow {
                offset: WindowOffset::Prev,
            },
            &mut svc,
            sid.clone(),
        );

        assert!(mutated, "FocusWindow Prev on 2-window session must mutate");
        assert_eq!(
            svc.sessions.get(&sid).unwrap().active_window,
            Some(w_last),
            "Prev from index 0 must wrap to last window"
        );
    }

    #[test]
    fn focus_window_in_single_window_session_returns_false() {
        use ozmux_configs::shortcuts::WindowOffset;

        let mut svc = MultiplexerService::default();
        let sid = svc.create_session(Some("default".into()));
        apply(Action::NewWindow, &mut svc, sid.clone());

        let active_before = svc
            .sessions
            .get(&sid)
            .unwrap()
            .active_window
            .clone()
            .unwrap();

        let mutated = apply(
            Action::FocusWindow {
                offset: WindowOffset::Next,
            },
            &mut svc,
            sid.clone(),
        );

        assert!(
            !mutated,
            "single-window session: FocusWindow Next must return false"
        );
        assert_eq!(
            svc.sessions.get(&sid).unwrap().active_window,
            Some(active_before),
            "active_window must not change"
        );
    }

    #[test]
    fn focus_window_last_returns_false_without_state_change() {
        use ozmux_configs::shortcuts::WindowOffset;

        let mut svc = MultiplexerService::default();
        let sid = svc.create_session(Some("default".into()));
        apply(Action::NewWindow, &mut svc, sid.clone());
        apply(Action::NewWindow, &mut svc, sid.clone());

        let active_before = svc
            .sessions
            .get(&sid)
            .unwrap()
            .active_window
            .clone()
            .unwrap();

        let mutated = apply(
            Action::FocusWindow {
                offset: WindowOffset::Last,
            },
            &mut svc,
            sid.clone(),
        );

        assert!(
            !mutated,
            "FocusWindow Last must return false (unimplemented)"
        );
        assert_eq!(
            svc.sessions.get(&sid).unwrap().active_window,
            Some(active_before),
            "Last must not change active_window"
        );
    }
}
