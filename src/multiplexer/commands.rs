//! Translates `configs::Action` into the matching multiplexer
//! `EntityEvent`. Called by the shortcut dispatcher in `src/input.rs`.
//!
//! Actions handled outside `dispatch()` (`NewSession`, `FocusSession`,
//! `FocusSessionNumber`, `EnterCopyMode`, `Copy`, `Paste`) are
//! short-circuited because they require entity-spawning, marker-moving,
//! or clipboard side effects the Bevy dispatcher performs directly.

use crate::multiplexer::commands::close_activity::{CloseActivityActionPlugin, CloseActivityEvent};
use crate::multiplexer::commands::close_pane::{ClosePaneActionPlugin, ClosePaneEvent};
use crate::multiplexer::commands::focus_activity::{FocusActivityActionPlugin, FocusActivityEvent};
use crate::multiplexer::commands::focus_pane::{FocusPaneActionPlugin, FocusPaneEvent};
use crate::multiplexer::commands::new_terminal_activity::{
    NewTerminalActivityActionPlugin, NewTerminalActivityEvent,
};
use crate::multiplexer::commands::split_pane::{SplitPaneActionPlugin, SplitPaneEvent};
use crate::multiplexer::commands::swap_pane::{SwapPaneActionPlugin, SwapPaneEvent};
use bevy::prelude::*;
use ozmux_configs::shortcuts::{
    ActivityOffset as ConfigActivityOffset, Direction as ConfigDirection, ShortcutAction,
    SplitDirection, SwapOffset as ConfigSwapOffset,
};
use ozmux_multiplexer::{CycleDirection, PaneDirection, SplitOrientation, SwapOffset};

pub mod close_activity;
pub mod close_pane;
pub mod focus_activity;
pub mod focus_pane;
pub mod new_terminal_activity;
pub mod split_pane;
pub mod swap_pane;

pub struct OzmuxShortcutActionPlugin;

impl Plugin for OzmuxShortcutActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            SplitPaneActionPlugin,
            NewTerminalActivityActionPlugin,
            FocusPaneActionPlugin,
            FocusActivityActionPlugin,
            SwapPaneActionPlugin,
            ClosePaneActionPlugin,
            CloseActivityActionPlugin,
        ));
    }
}

/// Translates a `configs::Action` into the matching multiplexer
/// EntityEvent and triggers it on `session`.
///
/// Actions handled outside `dispatch()` (`NewSession`, `FocusSession`,
/// `FocusSessionNumber`, `EnterCopyMode`, `Copy`, `Paste`) are handled by
/// explicit arms in the Bevy dispatcher (`src/input.rs`) and never reach
/// this function.
pub fn dispatch(commands: &mut Commands, action: ShortcutAction, session: Entity) {
    match action {
        ShortcutAction::SplitPane { direction } => {
            commands.trigger(SplitPaneEvent {
                session,
                orientation: split_orientation(direction),
            });
        }
        ShortcutAction::NewTerminalActivity => {
            commands.trigger(NewTerminalActivityEvent { session });
        }
        ShortcutAction::FocusPane { direction } => {
            commands.trigger(FocusPaneEvent {
                session,
                direction: focus_direction(direction),
            });
        }
        ShortcutAction::FocusActivity { offset } => {
            if let Some(direction) = cycle_direction(offset) {
                commands.trigger(FocusActivityEvent { session, direction });
            }
        }
        ShortcutAction::SwapPane { offset } => {
            commands.trigger(SwapPaneEvent {
                session,
                offset: swap_offset(offset),
            });
        }
        ShortcutAction::ClosePane => commands.trigger(ClosePaneEvent { session }),
        ShortcutAction::CloseActivity => commands.trigger(CloseActivityEvent { session }),
        ShortcutAction::NewSession
        | ShortcutAction::FocusSession { .. }
        | ShortcutAction::FocusSessionNumber { .. } => {}
        other => tracing::debug!(
            target: "ozmux_gui::commands",
            ?other,
            "shortcut action not yet implemented"
        ),
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

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use ozmux_configs::shortcuts::{
        ActivityOffset, Direction, SessionOffset, SwapOffset as CfgSwapOffset,
    };
    use ozmux_multiplexer::{MultiplexerCommands, MultiplexerPlugin, SessionMarker};

    /// Records which event observer fired. One enum variant per event
    /// type so dispatch translation tests can assert the right event
    /// was emitted without relying on the multiplexer side effects.
    #[derive(Debug, Default, Resource)]
    struct CapturedEvents(Vec<&'static str>);

    fn capture_split(_: On<SplitPaneEvent>, mut cap: ResMut<CapturedEvents>) {
        cap.0.push("SplitPane");
    }
    fn capture_new_activity(_: On<NewTerminalActivityEvent>, mut cap: ResMut<CapturedEvents>) {
        cap.0.push("NewTerminalActivity");
    }
    fn capture_focus_pane(_: On<FocusPaneEvent>, mut cap: ResMut<CapturedEvents>) {
        cap.0.push("FocusPane");
    }
    fn capture_focus_activity(_: On<FocusActivityEvent>, mut cap: ResMut<CapturedEvents>) {
        cap.0.push("FocusActivity");
    }
    fn capture_swap(_: On<SwapPaneEvent>, mut cap: ResMut<CapturedEvents>) {
        cap.0.push("SwapPane");
    }
    fn capture_close_pane(_: On<ClosePaneEvent>, mut cap: ResMut<CapturedEvents>) {
        cap.0.push("ClosePane");
    }
    fn capture_close_activity(_: On<CloseActivityEvent>, mut cap: ResMut<CapturedEvents>) {
        cap.0.push("CloseActivity");
    }

    fn setup_app() -> App {
        let mut app = App::new();
        app.add_plugins(MultiplexerPlugin);
        app.init_resource::<CapturedEvents>();
        app.add_observer(capture_split);
        app.add_observer(capture_new_activity);
        app.add_observer(capture_focus_pane);
        app.add_observer(capture_focus_activity);
        app.add_observer(capture_swap);
        app.add_observer(capture_close_pane);
        app.add_observer(capture_close_activity);
        app
    }

    fn bootstrap_session(world: &mut World) -> Entity {
        world
            .run_system_once(|mut mux: MultiplexerCommands| {
                mux.create_session(Some("test".into())).session
            })
            .unwrap()
    }

    fn run_dispatch(app: &mut App, action: ShortcutAction, session: Entity) {
        app.world_mut()
            .run_system_once(move |mut commands: Commands| {
                dispatch(&mut commands, action.clone(), session);
            })
            .unwrap();
        app.world_mut().flush();
    }

    fn captured(app: &App) -> Vec<&'static str> {
        app.world().resource::<CapturedEvents>().0.clone()
    }

    #[test]
    fn dispatch_split_pane_triggers_split_pane_event() {
        let mut app = setup_app();
        let session = bootstrap_session(app.world_mut());
        run_dispatch(
            &mut app,
            ShortcutAction::SplitPane {
                direction: SplitDirection::Horizontal,
            },
            session,
        );
        assert_eq!(captured(&app), vec!["SplitPane"]);
    }

    #[test]
    fn dispatch_new_terminal_activity_triggers_new_terminal_activity_event() {
        let mut app = setup_app();
        let session = bootstrap_session(app.world_mut());
        run_dispatch(&mut app, ShortcutAction::NewTerminalActivity, session);
        assert_eq!(captured(&app), vec!["NewTerminalActivity"]);
    }

    #[test]
    fn dispatch_focus_pane_triggers_focus_pane_event() {
        let mut app = setup_app();
        let session = bootstrap_session(app.world_mut());
        run_dispatch(
            &mut app,
            ShortcutAction::FocusPane {
                direction: Direction::Right,
            },
            session,
        );
        assert_eq!(captured(&app), vec!["FocusPane"]);
    }

    #[test]
    fn dispatch_focus_activity_next_triggers_focus_activity_event() {
        let mut app = setup_app();
        let session = bootstrap_session(app.world_mut());
        run_dispatch(
            &mut app,
            ShortcutAction::FocusActivity {
                offset: ActivityOffset::Next,
            },
            session,
        );
        assert_eq!(captured(&app), vec!["FocusActivity"]);
    }

    #[test]
    fn dispatch_focus_activity_last_emits_no_event() {
        let mut app = setup_app();
        let session = bootstrap_session(app.world_mut());
        run_dispatch(
            &mut app,
            ShortcutAction::FocusActivity {
                offset: ActivityOffset::Last,
            },
            session,
        );
        assert!(captured(&app).is_empty());
    }

    #[test]
    fn dispatch_swap_pane_triggers_swap_pane_event() {
        let mut app = setup_app();
        let session = bootstrap_session(app.world_mut());
        run_dispatch(
            &mut app,
            ShortcutAction::SwapPane {
                offset: CfgSwapOffset::Prev,
            },
            session,
        );
        assert_eq!(captured(&app), vec!["SwapPane"]);
    }

    #[test]
    fn dispatch_close_pane_triggers_close_pane_event() {
        let mut app = setup_app();
        let session = bootstrap_session(app.world_mut());
        run_dispatch(&mut app, ShortcutAction::ClosePane, session);
        assert_eq!(captured(&app), vec!["ClosePane"]);
    }

    #[test]
    fn dispatch_close_activity_triggers_close_activity_event() {
        let mut app = setup_app();
        let session = bootstrap_session(app.world_mut());
        run_dispatch(&mut app, ShortcutAction::CloseActivity, session);
        assert_eq!(captured(&app), vec!["CloseActivity"]);
    }

    #[test]
    fn dispatch_new_session_emits_no_event() {
        let mut app = setup_app();
        let session = bootstrap_session(app.world_mut());
        run_dispatch(&mut app, ShortcutAction::NewSession, session);
        assert!(captured(&app).is_empty());
    }

    #[test]
    fn dispatch_focus_session_emits_no_event() {
        let mut app = setup_app();
        let session = bootstrap_session(app.world_mut());
        run_dispatch(
            &mut app,
            ShortcutAction::FocusSession {
                offset: SessionOffset::Next,
            },
            session,
        );
        assert!(captured(&app).is_empty());
    }

    #[test]
    fn dispatch_focus_session_number_emits_no_event() {
        let mut app = setup_app();
        let session = bootstrap_session(app.world_mut());
        run_dispatch(
            &mut app,
            ShortcutAction::FocusSessionNumber { index: 0 },
            session,
        );
        assert!(captured(&app).is_empty());
    }

    #[test]
    fn dispatch_unknown_action_emits_no_event() {
        let mut app = setup_app();
        let session = bootstrap_session(app.world_mut());
        run_dispatch(&mut app, ShortcutAction::ZoomPane, session);
        assert!(captured(&app).is_empty());
    }

    #[test]
    fn dispatch_on_vanished_session_emits_event_without_panic() {
        let mut app = setup_app();
        let bogus = app.world_mut().spawn(SessionMarker).id();
        app.world_mut().despawn(bogus);
        app.world_mut().flush();
        run_dispatch(&mut app, ShortcutAction::ClosePane, bogus);
        assert_eq!(captured(&app), vec!["ClosePane"]);
    }
}
