//! Keyboard shortcut handling: PrefixState Component and dispatcher systems.
//! `Action` enum lives under `input/action.rs`.

use bevy::prelude::*;
use bevy::time::{Timer, TimerMode};
use bevy_egui::input::EguiWantsInput;

/// `Action` enum produced by the shortcut dispatcher.
pub mod action;

/// Per-GUI-window prefix-mode state. `armed` flips to true the frame
/// Ctrl-B is pressed and the 2-second `timeout` is reset; it flips back to
/// false when the timeout expires, an `Action` fires, or a non-modifier key
/// cancels the prefix.
#[derive(Component, Debug)]
pub struct PrefixState {
    pub(crate) armed: bool,
    pub(crate) timeout: Timer,
}

impl Default for PrefixState {
    fn default() -> Self {
        Self {
            armed: false,
            timeout: Timer::from_seconds(2.0, TimerMode::Once),
        }
    }
}

/// Bevy Plugin that registers the keyboard shortcut handling pipeline:
/// `tick_prefix_state` (Stage A) and `dispatch_focused_key` (Stage B) chained
/// in the `Update` schedule.
pub struct OzmuxShortcutPlugin;

impl Plugin for OzmuxShortcutPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                tick_prefix_state,
                dispatch_focused_key.run_if(egui_want_keyboard_input),
            )
                .chain(),
        );
    }
}

/// Advance every armed `PrefixState`'s timer; flip `armed` off when
/// the timer expires. Runs for *all* GUI windows regardless of focus, so a
/// detached window still expires naturally.
fn tick_prefix_state(time: Res<Time<Virtual>>, mut q: Query<&mut PrefixState>) {
    for mut prefix in &mut q {
        if !prefix.armed {
            continue;
        }
        prefix.timeout.tick(time.delta());
        if prefix.timeout.is_finished() {
            prefix.armed = false;
        }
    }
}

pub(crate) fn dispatch_focused_key(
    keys: Res<ButtonInput<KeyCode>>,
    mut mux: ResMut<crate::multiplexer::Multiplexer>,
    mut q: Query<(
        &crate::multiplexer::AttachedSession,
        &mut PrefixState,
        &Window,
    )>,
) {
    for (attached, mut prefix, win) in &mut q {
        if !win.focused {
            continue;
        }
        if !prefix.armed {
            if (keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight))
                && keys.just_pressed(KeyCode::KeyB)
            {
                prefix.armed = true;
                prefix.timeout.reset();
            }
            continue;
        }
        if let Some(action) = decode_action(&keys) {
            let mux_ref = mux.bypass_change_detection();
            let mutated = crate::multiplexer::commands::apply(action, mux_ref, attached.0.clone());
            if mutated {
                mux.set_changed();
            }
            prefix.armed = false;
        } else if just_pressed_non_modifier(&keys) {
            prefix.armed = false;
        }
    }
}

fn egui_want_keyboard_input(egui_wants: Option<Res<EguiWantsInput>>) -> bool {
    !egui_wants.is_some_and(|e| e.wants_keyboard_input())
}

fn just_pressed_non_modifier(keys: &ButtonInput<KeyCode>) -> bool {
    keys.get_just_pressed().any(|k| !is_modifier(*k))
}

fn is_modifier(k: KeyCode) -> bool {
    matches!(
        k,
        KeyCode::ShiftLeft
            | KeyCode::ShiftRight
            | KeyCode::ControlLeft
            | KeyCode::ControlRight
            | KeyCode::AltLeft
            | KeyCode::AltRight
            | KeyCode::SuperLeft
            | KeyCode::SuperRight
    )
}

fn decode_action(keys: &ButtonInput<KeyCode>) -> Option<action::Action> {
    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    if keys.just_pressed(KeyCode::KeyC) {
        return Some(action::Action::NewWindow);
    }
    if shift && keys.just_pressed(KeyCode::Quote) {
        return Some(action::Action::SplitPaneHorizontal);
    }
    if shift && keys.just_pressed(KeyCode::Digit5) {
        return Some(action::Action::SplitPaneVertical);
    }
    if keys.just_pressed(KeyCode::KeyT) {
        return Some(action::Action::NewActivity);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_prefix_state_starts_disarmed_with_2s_timer() {
        let p = PrefixState::default();
        assert!(!p.armed);
        assert_eq!(p.timeout.duration().as_secs(), 2);
        assert_eq!(p.timeout.mode(), TimerMode::Once);
    }

    #[test]
    fn tick_prefix_state_expires_armed_after_timeout() {
        use bevy::time::TimeUpdateStrategy;
        use std::time::Duration;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_systems(Update, tick_prefix_state);

        let entity = app
            .world_mut()
            .spawn(PrefixState {
                armed: true,
                timeout: Timer::from_seconds(2.0, TimerMode::Once),
            })
            .id();

        app.world_mut()
            .resource_mut::<Time<Virtual>>()
            .set_max_delta(Duration::from_secs(60));

        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::ZERO));
        app.update();
        assert!(
            app.world().get::<PrefixState>(entity).unwrap().armed,
            "armed must still be true before tick"
        );

        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs(3)));
        app.update();

        assert!(
            !app.world().get::<PrefixState>(entity).unwrap().armed,
            "armed must flip to false after timer expires"
        );
    }

    #[test]
    fn tick_prefix_state_leaves_disarmed_alone() {
        use bevy::time::TimeUpdateStrategy;
        use std::time::Duration;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_systems(Update, tick_prefix_state);

        let entity = app.world_mut().spawn(PrefixState::default()).id();
        app.world_mut()
            .resource_mut::<Time<Virtual>>()
            .set_max_delta(Duration::from_secs(60));
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs(10)));
        app.update();

        let p = app.world().get::<PrefixState>(entity).unwrap();
        assert!(!p.armed);
    }

    #[test]
    fn ctrl_b_arms_prefix_state_on_focused_window() {
        use bevy::window::{Window, WindowResolution};

        use crate::multiplexer::{AttachedSession, Multiplexer, OzmuxMultiplexerPlugin};
        use ozmux_multiplexer::SessionId;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(OzmuxMultiplexerPlugin)
            .add_systems(Update, dispatch_focused_key);

        app.insert_resource(ButtonInput::<KeyCode>::default());

        let entity = app
            .world_mut()
            .spawn((
                Window {
                    focused: true,
                    resolution: WindowResolution::new(800, 600),
                    ..default()
                },
                AttachedSession(SessionId::new()),
                PrefixState::default(),
            ))
            .id();

        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.press(KeyCode::ControlLeft);
            keys.press(KeyCode::KeyB);
        }
        app.update();

        let p = app.world().get::<PrefixState>(entity).unwrap();
        assert!(p.armed, "Ctrl-B must arm the prefix state");
        assert!(!p.timeout.is_finished());

        assert!(
            app.world().resource::<Multiplexer>().sessions.is_empty(),
            "arming alone must not mutate domain state"
        );
    }

    #[test]
    fn ctrl_b_on_unfocused_window_does_not_arm() {
        use bevy::window::{Window, WindowResolution};

        use crate::multiplexer::{AttachedSession, OzmuxMultiplexerPlugin};
        use ozmux_multiplexer::SessionId;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(OzmuxMultiplexerPlugin)
            .add_systems(Update, dispatch_focused_key);

        app.insert_resource(ButtonInput::<KeyCode>::default());

        let entity = app
            .world_mut()
            .spawn((
                Window {
                    focused: false,
                    resolution: WindowResolution::new(800, 600),
                    ..default()
                },
                AttachedSession(SessionId::new()),
                PrefixState::default(),
            ))
            .id();

        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.press(KeyCode::ControlLeft);
            keys.press(KeyCode::KeyB);
        }
        app.update();

        assert!(!app.world().get::<PrefixState>(entity).unwrap().armed);
    }

    #[test]
    fn armed_then_c_fires_new_window() {
        use bevy::window::{Window, WindowResolution};

        use crate::multiplexer::{AttachedSession, Multiplexer, OzmuxMultiplexerPlugin};

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(OzmuxMultiplexerPlugin)
            .add_systems(Update, dispatch_focused_key);
        app.insert_resource(ButtonInput::<KeyCode>::default());

        let sid = {
            let mut mux = app.world_mut().resource_mut::<Multiplexer>();
            mux.create_session(Some("default".into()))
        };

        app.world_mut().spawn((
            Window {
                focused: true,
                resolution: WindowResolution::new(800, 600),
                ..default()
            },
            AttachedSession(sid.clone()),
            PrefixState {
                armed: true,
                timeout: Timer::from_seconds(2.0, TimerMode::Once),
            },
        ));

        let windows_before = app.world().resource::<Multiplexer>().windows.len();

        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.press(KeyCode::KeyC);
        }
        app.update();

        let mux = app.world().resource::<Multiplexer>();
        assert_eq!(mux.windows.len(), windows_before + 1);
    }

    #[test]
    fn armed_then_unrelated_key_cancels_without_firing() {
        use bevy::window::{Window, WindowResolution};

        use crate::multiplexer::{AttachedSession, Multiplexer, OzmuxMultiplexerPlugin};
        use ozmux_multiplexer::SessionId;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(OzmuxMultiplexerPlugin)
            .add_systems(Update, dispatch_focused_key);
        app.insert_resource(ButtonInput::<KeyCode>::default());

        let entity = app
            .world_mut()
            .spawn((
                Window {
                    focused: true,
                    resolution: WindowResolution::new(800, 600),
                    ..default()
                },
                AttachedSession(SessionId::new()),
                PrefixState {
                    armed: true,
                    timeout: Timer::from_seconds(2.0, TimerMode::Once),
                },
            ))
            .id();

        let windows_before = app.world().resource::<Multiplexer>().windows.len();
        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.press(KeyCode::KeyZ);
        }
        app.update();

        let p = app.world().get::<PrefixState>(entity).unwrap();
        assert!(!p.armed, "unrelated key must disarm");
        let mux = app.world().resource::<Multiplexer>();
        assert_eq!(
            mux.windows.len(),
            windows_before,
            "unrelated key must not mutate Multiplexer"
        );
    }

    #[test]
    fn shift_press_alone_does_not_disarm_prefix() {
        use bevy::window::{Window, WindowResolution};

        use crate::multiplexer::{AttachedSession, OzmuxMultiplexerPlugin};
        use ozmux_multiplexer::SessionId;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(OzmuxMultiplexerPlugin)
            .add_systems(Update, dispatch_focused_key);
        app.insert_resource(ButtonInput::<KeyCode>::default());

        let entity = app
            .world_mut()
            .spawn((
                Window {
                    focused: true,
                    resolution: WindowResolution::new(800, 600),
                    ..default()
                },
                AttachedSession(SessionId::new()),
                PrefixState {
                    armed: true,
                    timeout: Timer::from_seconds(2.0, TimerMode::Once),
                },
            ))
            .id();

        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.press(KeyCode::ShiftLeft);
        }
        app.update();

        assert!(
            app.world().get::<PrefixState>(entity).unwrap().armed,
            "Shift alone must not disarm; it is a modifier needed for \" and %"
        );
    }

    #[test]
    fn armed_then_shift_quote_fires_split_horizontal() {
        use bevy::window::{Window, WindowResolution};

        use crate::multiplexer::{AttachedSession, Multiplexer, OzmuxMultiplexerPlugin};

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(OzmuxMultiplexerPlugin)
            .add_systems(Update, dispatch_focused_key);
        app.insert_resource(ButtonInput::<KeyCode>::default());

        let sid = {
            let mut mux = app.world_mut().resource_mut::<Multiplexer>();
            let sid = mux.create_session(Some("default".into()));
            mux.create_window(Some(&sid), Some("main".into())).unwrap();
            sid
        };

        app.world_mut().spawn((
            Window {
                focused: true,
                resolution: WindowResolution::new(800, 600),
                ..default()
            },
            AttachedSession(sid.clone()),
            PrefixState {
                armed: true,
                timeout: Timer::from_seconds(2.0, TimerMode::Once),
            },
        ));

        let wid = app
            .world()
            .resource::<Multiplexer>()
            .sessions
            .get(&sid)
            .unwrap()
            .linked_windows[0]
            .clone();
        let panes_before = app
            .world()
            .resource::<Multiplexer>()
            .windows
            .get(&wid)
            .unwrap()
            .pane_ids()
            .count();

        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.press(KeyCode::ShiftLeft);
        }
        app.update();

        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.press(KeyCode::Quote);
        }
        app.update();

        let panes_after = app
            .world()
            .resource::<Multiplexer>()
            .windows
            .get(&wid)
            .unwrap()
            .pane_ids()
            .count();
        assert_eq!(
            panes_after,
            panes_before + 1,
            "Shift held + Quote pressed must fire SplitPaneHorizontal"
        );
    }

    #[test]
    fn shortcut_plugin_registers_systems_without_panic() {
        use crate::multiplexer::OzmuxMultiplexerPlugin;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(OzmuxMultiplexerPlugin)
            .add_plugins(OzmuxShortcutPlugin);
        app.insert_resource(ButtonInput::<KeyCode>::default());
        app.update();
    }
}
