//! Pure (Bevy-independent) functions that apply a `configs::Action` to the
//! domain `MultiplexerService`. Called by the shortcut dispatcher.

use ozmux_configs::shortcuts::{Action, SplitDirection};
use ozmux_multiplexer::{
    Activity, ActivityId, MultiplexerService, PaneId, SessionId, Side, SplitOrientation,
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
    let outcome = mux.with_window(
        &active_window,
        |w| -> Result<(), ozmux_multiplexer::MultiplexerError> {
            let pane = w.pane_mut(&active_pane)?;
            pane.add_activity(activity)?;
            let _ = pane.set_active_activity(&new_id)?;
            Ok(())
        },
    );
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
        use ozmux_configs::shortcuts::Direction;
        let mut svc = MultiplexerService::default();
        let sid = svc.create_session(Some("default".into()));
        apply(Action::NewWindow, &mut svc, sid.clone());

        let windows_before = svc.windows.len();

        let mutated = apply(
            Action::FocusPane {
                direction: Direction::Left,
            },
            &mut svc,
            sid,
        );

        assert!(!mutated, "unimplemented variant must return false");
        assert_eq!(svc.windows.len(), windows_before);
    }
}
