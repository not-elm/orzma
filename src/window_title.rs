//! Dynamic OS window-title sync: reflects the active context per `AppMode`
//! into the primary window's title bar — `session:window — ozmux` in Ozmux
//! mode, the focused terminal's OSC title + ` — ozmux` in Ozma mode.

use crate::app_mode::AppMode;
use bevy::prelude::*;
use bevy::window::{PrimaryWindow, Window};
use ozma_terminal::{KeyboardFocused, OzmaTerminal};
use ozma_tty_engine::{TerminalTitle, sanitize_title};
use ozmux_tmux::{ActiveWindow, TmuxProjectionSet, TmuxSession, TmuxWindow};

/// Keeps the primary OS window title in sync with the active `AppMode`
/// context: the tmux `session:window` in Ozmux mode, and the focused
/// terminal's OSC title in Ozma mode.
pub(crate) struct WindowTitlePlugin;

impl Plugin for WindowTitlePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                update_default_window_title.run_if(in_state(AppMode::Default)),
                update_tmux_window_title
                    .run_if(in_state(AppMode::Tmux))
                    .run_if(ozmux_title_dirty)
                    .after(TmuxProjectionSet),
            ),
        )
        .add_systems(OnEnter(AppMode::Tmux), update_tmux_window_title);
    }
}

const APP_NAME: &str = "ozmux";

const SUFFIX: &str = " — ozmux";

fn update_default_window_title(
    mut window: Query<&mut Window, With<PrimaryWindow>>,
    focused: Query<&TerminalTitle, (With<OzmaTerminal>, With<KeyboardFocused>)>,
    terminals: Query<(), With<OzmaTerminal>>,
) {
    let Ok(mut window) = window.single_mut() else {
        return;
    };
    // NOTE: the no-focus branch is deliberately asymmetric. Hold the last title
    // when terminals exist but focus is transiently absent (a handoff — avoids a
    // one-frame flicker); reset to the app-name fallback when no terminal exists
    // at all, so a stale cross-mode title cannot linger after entering Ozma
    // before the deferred terminal spawn flushes.
    if let Ok(title) = focused.single() {
        apply_title(&mut window, format_ozma(title.0.as_deref()));
    } else if terminals.is_empty() {
        apply_title(&mut window, format_ozma(None));
    }
}

fn update_tmux_window_title(
    mut window: Query<&mut Window, With<PrimaryWindow>>,
    sessions: Query<&TmuxSession>,
    active_windows: Query<&TmuxWindow, With<ActiveWindow>>,
) {
    let Ok(mut window) = window.single_mut() else {
        return;
    };
    // NOTE: tmux session/window names are untrusted and bypass the OSC title
    // sanitizer, so sanitize them here before they reach the OS title bar.
    let session = sessions
        .iter()
        .next()
        .map(|s| sanitize_title(&s.name))
        .unwrap_or_default();
    let active = active_windows
        .iter()
        .next()
        .map(|w| sanitize_title(&w.name));
    apply_title(&mut window, format_ozmux(&session, active.as_deref()));
}

/// True when the tmux session name, the active window, or the active window's
/// name may have changed this frame — the inputs to the Ozmux window title.
fn ozmux_title_dirty(
    mut removed_session: RemovedComponents<TmuxSession>,
    mut removed_active: RemovedComponents<ActiveWindow>,
    changed_session: Query<(), Changed<TmuxSession>>,
    changed_active_window: Query<(), (Changed<TmuxWindow>, With<ActiveWindow>)>,
    added_active: Query<(), Added<ActiveWindow>>,
) -> bool {
    // NOTE: drain both RemovedComponents readers up front, not inside the `||`
    // chain — a short-circuit on an earlier term would leave the one-frame
    // removal events unread, so they would re-fire (a spurious run) next frame.
    let session_removed = removed_session.read().next().is_some();
    let active_removed = removed_active.read().next().is_some();
    !changed_session.is_empty()
        || !changed_active_window.is_empty()
        || !added_active.is_empty()
        || session_removed
        || active_removed
}

fn format_ozma(title: Option<&str>) -> String {
    match title.map(str::trim) {
        Some(t) if !t.is_empty() => format!("{t}{SUFFIX}"),
        _ => APP_NAME.to_string(),
    }
}

fn format_ozmux(session: &str, window: Option<&str>) -> String {
    let session = session.trim();
    if session.is_empty() {
        return APP_NAME.to_string();
    }
    match window.map(str::trim) {
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

    #[test]
    fn ozma_whitespace_only_title_is_app_name() {
        assert_eq!(format_ozma(Some("   ")), "ozmux");
    }

    #[test]
    fn ozma_trims_surrounding_whitespace() {
        assert_eq!(format_ozma(Some("  vim  ")), "vim — ozmux");
    }

    #[test]
    fn ozmux_whitespace_session_is_app_name() {
        assert_eq!(format_ozmux("   ", Some("vim")), "ozmux");
    }

    #[test]
    fn ozmux_trims_session_and_window() {
        assert_eq!(
            format_ozmux("  main  ", Some("  vim  ")),
            "main:vim — ozmux"
        );
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
        app.insert_state(AppMode::Default);
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
        app.insert_state(AppMode::Tmux);
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

    #[test]
    fn ozma_resets_to_app_name_when_no_terminal_exists() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin));
        app.insert_state(AppMode::Default);
        app.add_plugins(WindowTitlePlugin);
        app.world_mut().spawn((
            Window {
                title: "main:vim — ozmux".to_string(),
                ..default()
            },
            PrimaryWindow,
        ));

        app.update();

        assert_eq!(primary_window_title(&mut app), "ozmux");
    }

    #[test]
    fn ozma_holds_last_title_when_terminal_exists_but_unfocused() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin));
        app.insert_state(AppMode::Default);
        app.add_plugins(WindowTitlePlugin);
        app.world_mut().spawn((
            Window {
                title: "held — ozmux".to_string(),
                ..default()
            },
            PrimaryWindow,
        ));
        app.world_mut()
            .spawn((OzmaTerminal, TerminalTitle(Some("vim".to_string()))));

        app.update();

        assert_eq!(primary_window_title(&mut app), "held — ozmux");
    }

    #[test]
    fn ozmux_sanitizes_window_name() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin));
        app.insert_state(AppMode::Tmux);
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
                name: "vi\u{7}m".to_string(),
            },
            ActiveWindow,
        ));

        app.update();

        assert_eq!(primary_window_title(&mut app), "main:vim — ozmux");
    }

    #[test]
    fn ozmux_title_is_not_recomputed_when_nothing_changed() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin));
        app.insert_state(AppMode::Tmux);
        app.add_plugins(WindowTitlePlugin);
        app.world_mut().spawn((Window::default(), PrimaryWindow));
        let session = app
            .world_mut()
            .spawn(TmuxSession {
                id: SessionId(1),
                name: "main".to_string(),
            })
            .id();
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

        // An unchanged frame must not recompute the title: overwrite it out of
        // band, update with no tmux change, and confirm the gate suppressed the
        // system (the sentinel survives).
        {
            let world = app.world_mut();
            let mut windows = world.query_filtered::<&mut Window, With<PrimaryWindow>>();
            windows.single_mut(world).unwrap().title = "SENTINEL".to_string();
        }
        app.update();
        assert_eq!(primary_window_title(&mut app), "SENTINEL");

        // Renaming the session marks TmuxSession Changed, so the gate fires again.
        app.world_mut()
            .entity_mut(session)
            .get_mut::<TmuxSession>()
            .unwrap()
            .name = "other".to_string();
        app.update();
        assert_eq!(primary_window_title(&mut app), "other:vim — ozmux");
    }
}
