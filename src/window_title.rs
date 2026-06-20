//! Dynamic OS window-title sync: reflects the active context per `AppMode`
//! into the primary window's title bar — `session:window — ozmux` in Ozmux
//! mode, the focused terminal's OSC title + ` — ozmux` in Ozma mode.

use crate::ozma::AppMode;
use bevy::prelude::*;
use bevy::window::{PrimaryWindow, Window};
use ozma_terminal::{KeyboardFocused, OzmaTerminal};
use ozma_tty_engine::TerminalTitle;
use ozmux_tmux::{ActiveWindow, TmuxSession, TmuxWindow};

/// Keeps the primary OS window title in sync with the active `AppMode`
/// context: the tmux `session:window` in Ozmux mode, and the focused
/// terminal's OSC title in Ozma mode.
pub(crate) struct WindowTitlePlugin;

impl Plugin for WindowTitlePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                update_ozma_window_title.run_if(in_state(AppMode::Ozma)),
                update_ozmux_window_title.run_if(in_state(AppMode::Ozmux)),
            ),
        );
    }
}

const APP_NAME: &str = "ozmux";

const SUFFIX: &str = " — ozmux";

fn update_ozma_window_title(
    mut window: Query<&mut Window, With<PrimaryWindow>>,
    focused: Query<&TerminalTitle, (With<OzmaTerminal>, With<KeyboardFocused>)>,
) {
    let Ok(mut window) = window.single_mut() else {
        return;
    };
    let Ok(title) = focused.single() else {
        return;
    };
    apply_title(&mut window, format_ozma(title.0.as_deref()));
}

fn update_ozmux_window_title(
    mut window: Query<&mut Window, With<PrimaryWindow>>,
    sessions: Query<&TmuxSession>,
    active_windows: Query<&TmuxWindow, With<ActiveWindow>>,
) {
    let Ok(mut window) = window.single_mut() else {
        return;
    };
    let session = sessions
        .iter()
        .next()
        .map(|s| s.name.as_str())
        .unwrap_or("");
    let active = active_windows.iter().next().map(|w| w.name.as_str());
    apply_title(&mut window, format_ozmux(session, active));
}

fn format_ozma(title: Option<&str>) -> String {
    match title {
        Some(t) if !t.is_empty() => format!("{t}{SUFFIX}"),
        _ => APP_NAME.to_string(),
    }
}

fn format_ozmux(session: &str, window: Option<&str>) -> String {
    if session.is_empty() {
        return APP_NAME.to_string();
    }
    match window {
        Some(w) if !w.is_empty() => format!("{session}:{w}{SUFFIX}"),
        _ => format!("{session}{SUFFIX}"),
    }
}

fn apply_title(window: &mut Window, desired: String) {
    if window.title != desired {
        window.title = desired;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::state::app::StatesPlugin;
    use ozmux_tmux::{SessionId, WindowId};

    #[test]
    fn ozma_some_title_gets_suffix() {
        assert_eq!(format_ozma(Some("vim")), "vim — ozmux");
    }

    #[test]
    fn ozma_empty_title_is_app_name() {
        assert_eq!(format_ozma(Some("")), "ozmux");
    }

    #[test]
    fn ozma_none_title_is_app_name() {
        assert_eq!(format_ozma(None), "ozmux");
    }

    #[test]
    fn ozmux_session_and_window() {
        assert_eq!(format_ozmux("main", Some("vim")), "main:vim — ozmux");
    }

    #[test]
    fn ozmux_session_only_when_window_absent() {
        assert_eq!(format_ozmux("main", None), "main — ozmux");
    }

    #[test]
    fn ozmux_session_only_when_window_empty() {
        assert_eq!(format_ozmux("main", Some("")), "main — ozmux");
    }

    #[test]
    fn ozmux_empty_session_is_app_name() {
        assert_eq!(format_ozmux("", Some("vim")), "ozmux");
        assert_eq!(format_ozmux("", None), "ozmux");
    }

    fn primary_window_title(app: &mut App) -> String {
        let world = app.world_mut();
        let mut windows = world.query_filtered::<&Window, With<PrimaryWindow>>();
        windows
            .iter(world)
            .next()
            .expect("primary window exists")
            .title
            .clone()
    }

    #[test]
    fn ozma_system_sets_focused_terminal_title() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin));
        app.insert_state(AppMode::Ozma);
        app.add_plugins(WindowTitlePlugin);
        app.world_mut().spawn((Window::default(), PrimaryWindow));
        app.world_mut().spawn((
            OzmaTerminal,
            KeyboardFocused,
            TerminalTitle(Some("vim".to_string())),
        ));

        app.update();

        assert_eq!(primary_window_title(&mut app), "vim — ozmux");
    }

    #[test]
    fn ozmux_system_sets_session_and_active_window() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin));
        app.insert_state(AppMode::Ozmux);
        app.add_plugins(WindowTitlePlugin);
        app.world_mut().spawn((Window::default(), PrimaryWindow));
        app.world_mut().spawn(TmuxSession {
            id: SessionId(1),
            name: "main".to_string(),
        });
        app.world_mut().spawn((
            TmuxWindow {
                id: WindowId(2),
                index: 1,
                name: "vim".to_string(),
            },
            ActiveWindow,
        ));

        app.update();

        assert_eq!(primary_window_title(&mut app), "main:vim — ozmux");
    }
}
