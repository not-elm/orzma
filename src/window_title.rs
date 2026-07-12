//! Dynamic OS window-title sync: reflects the focused terminal's OSC title
//! (+ ` — orzma`) into the primary window's title bar.

use crate::input::focus::KeyboardFocused;
use crate::surface::OrzmaTerminal;
use bevy::prelude::*;
use bevy::window::{PrimaryWindow, Window};
use orzma_tty_engine::TerminalTitle;

/// Keeps the primary OS window title in sync with the focused terminal's OSC
/// title.
pub(crate) struct WindowTitlePlugin;

impl Plugin for WindowTitlePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, update_default_window_title);
    }
}

const APP_NAME: &str = "orzma";

const SUFFIX: &str = " — orzma";

fn update_default_window_title(
    mut window: Query<&mut Window, With<PrimaryWindow>>,
    focused: Query<&TerminalTitle, (With<OrzmaTerminal>, With<KeyboardFocused>)>,
    terminals: Query<(), With<OrzmaTerminal>>,
) {
    let Ok(mut window) = window.single_mut() else {
        return;
    };
    // NOTE: the no-focus branch is deliberately asymmetric. Hold the last title
    // when terminals exist but focus is transiently absent (a handoff — avoids a
    // one-frame flicker); reset to the app-name fallback when no terminal exists
    // at all, so a stale title cannot linger before the deferred terminal spawn
    // flushes.
    if let Ok(title) = focused.single() {
        apply_title(&mut window, format_default(title.0.as_deref()));
    } else if terminals.is_empty() {
        apply_title(&mut window, format_default(None));
    }
}

fn format_default(title: Option<&str>) -> String {
    match title.map(str::trim) {
        Some(t) if !t.is_empty() => format!("{t}{SUFFIX}"),
        _ => APP_NAME.to_string(),
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

    #[test]
    fn default_some_title_gets_suffix() {
        assert_eq!(format_default(Some("vim")), "vim — orzma");
    }

    #[test]
    fn default_empty_title_is_app_name() {
        assert_eq!(format_default(Some("")), "orzma");
    }

    #[test]
    fn default_none_title_is_app_name() {
        assert_eq!(format_default(None), "orzma");
    }

    #[test]
    fn default_whitespace_only_title_is_app_name() {
        assert_eq!(format_default(Some("   ")), "orzma");
    }

    #[test]
    fn default_trims_surrounding_whitespace() {
        assert_eq!(format_default(Some("  vim  ")), "vim — orzma");
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
    fn default_system_sets_focused_terminal_title() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(WindowTitlePlugin);
        app.world_mut().spawn((Window::default(), PrimaryWindow));
        app.world_mut().spawn((
            OrzmaTerminal,
            KeyboardFocused,
            TerminalTitle(Some("vim".to_string())),
        ));

        app.update();

        assert_eq!(primary_window_title(&mut app), "vim — orzma");
    }

    #[test]
    fn default_resets_to_app_name_when_no_terminal_exists() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(WindowTitlePlugin);
        app.world_mut().spawn((
            Window {
                title: "main:vim — orzma".to_string(),
                ..default()
            },
            PrimaryWindow,
        ));

        app.update();

        assert_eq!(primary_window_title(&mut app), "orzma");
    }

    #[test]
    fn default_holds_last_title_when_terminal_exists_but_unfocused() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(WindowTitlePlugin);
        app.world_mut().spawn((
            Window {
                title: "held — orzma".to_string(),
                ..default()
            },
            PrimaryWindow,
        ));
        app.world_mut()
            .spawn((OrzmaTerminal, TerminalTitle(Some("vim".to_string()))));

        app.update();

        assert_eq!(primary_window_title(&mut app), "held — orzma");
    }
}
