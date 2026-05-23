//! Keyboard shortcut handling: `PrefixState` Component and dispatcher
//! systems. The shortcut binding table (prefix + bindings) comes from
//! the loaded `OzmuxConfigsResource`; this module owns no chord data.

use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use bevy::time::{Timer, TimerMode};
use ozmux_configs::shortcuts::{KeyChord, Modifiers, Prefix, Shortcuts};
use std::time::Duration;

/// Per-GUI-window prefix-mode state. `armed` flips to true the frame the
/// configured prefix chord is pressed and the configured timeout is reset;
/// it flips back to false when the timeout expires, a binding fires, or a
/// non-modifier key cancels the prefix.
#[derive(Component, Debug)]
pub struct PrefixState {
    pub(crate) armed: bool,
    pub(crate) timeout: Timer,
}

impl PrefixState {
    /// Builds a fresh `PrefixState` whose timeout is sourced from the
    /// `Shortcuts::prefix.timeout_ms` value (rather than a hard-coded
    /// default).
    pub fn from_prefix(prefix: &Prefix) -> Self {
        Self {
            armed: false,
            timeout: Timer::new(Duration::from_millis(prefix.timeout_ms), TimerMode::Once),
        }
    }
}

/// Bevy Plugin that registers the keyboard shortcut handling pipeline:
/// `tick_prefix_state` (Stage A) and `dispatch_focused_key` (Stage B)
/// chained in the `Update` schedule. No focus gating — the migrated UI
/// has no text inputs that consume keyboard focus.
pub struct OzmuxShortcutPlugin;

impl Plugin for OzmuxShortcutPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (tick_prefix_state, dispatch_focused_key).chain());
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
    mut events: MessageReader<KeyboardInput>,
    keys: Res<ButtonInput<KeyCode>>,
    configs: Res<crate::configs::OzmuxConfigsResource>,
    mut mux: ResMut<crate::multiplexer::Multiplexer>,
    mut q: Query<(
        &crate::multiplexer::AttachedSession,
        &mut PrefixState,
        &Window,
    )>,
) {
    let shortcuts = &configs.shortcuts;
    let mods = current_modifiers(&keys);

    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        let Ok((attached, mut prefix, win)) = q.get_mut(ev.window) else {
            continue;
        };
        if !win.focused {
            continue;
        }
        if is_modifier_only_key(&ev.logical_key) {
            continue;
        }
        match bevy_to_configs_key(&ev.logical_key) {
            Some(input_key) => {
                handle_chord(
                    &input_key,
                    &mods,
                    &mut prefix,
                    shortcuts,
                    &mut mux,
                    attached,
                );
            }
            None => prefix.armed = false,
        }
    }
}

fn current_modifiers(keys: &ButtonInput<KeyCode>) -> Modifiers {
    Modifiers {
        ctrl: keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight),
        shift: keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight),
        alt: keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight),
        meta: keys.pressed(KeyCode::SuperLeft) || keys.pressed(KeyCode::SuperRight),
    }
}

fn match_chord(
    input_key: &ozmux_configs::shortcuts::Key,
    mods: &Modifiers,
    chord: &KeyChord,
) -> bool {
    input_key == &chord.key && mods == &chord.modifiers
}

fn mods_subtract(current: &Modifiers, to_remove: &Modifiers) -> Modifiers {
    Modifiers {
        ctrl: current.ctrl && !to_remove.ctrl,
        shift: current.shift && !to_remove.shift,
        alt: current.alt && !to_remove.alt,
        meta: current.meta && !to_remove.meta,
    }
}

fn is_modifier_only_key(key: &Key) -> bool {
    // Only keys that are HELD WHILE the chord follow-up is typed should bypass
    // the disarm logic. Toggle-style lock keys (CapsLock / NumLock / ScrollLock /
    // FnLock / SymbolLock) are intentional discrete presses and should disarm,
    // matching the original `is_modifier` set in `src/input.rs` pre-rewrite.
    matches!(
        key,
        Key::Shift
            | Key::Control
            | Key::Alt
            | Key::Super
            | Key::Meta
            | Key::Hyper
            | Key::AltGraph
            | Key::Fn
            | Key::Symbol
    )
}

fn bevy_to_configs_key(key: &Key) -> Option<ozmux_configs::shortcuts::Key> {
    use ozmux_configs::shortcuts::Key as CKey;
    Some(match key {
        Key::Character(s) => {
            let mut chars = s.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            let normalized = if c.is_ascii_alphabetic() {
                c.to_ascii_lowercase()
            } else {
                c
            };
            CKey::Char(normalized)
        }
        Key::Escape => CKey::Escape,
        Key::Enter => CKey::Enter,
        Key::Tab => CKey::Tab,
        Key::Backspace => CKey::Backspace,
        Key::Space => CKey::Space,
        Key::ArrowUp => CKey::ArrowUp,
        Key::ArrowDown => CKey::ArrowDown,
        Key::ArrowLeft => CKey::ArrowLeft,
        Key::ArrowRight => CKey::ArrowRight,
        _ => return None,
    })
}

fn handle_chord(
    input_key: &ozmux_configs::shortcuts::Key,
    mods: &Modifiers,
    prefix: &mut PrefixState,
    shortcuts: &Shortcuts,
    mux: &mut ResMut<crate::multiplexer::Multiplexer>,
    attached: &crate::multiplexer::AttachedSession,
) {
    if !prefix.armed {
        if match_chord(input_key, mods, &shortcuts.prefix.chord) {
            prefix.armed = true;
            prefix.timeout.reset();
        }
        return;
    }
    let mods_without_prefix = mods_subtract(mods, &shortcuts.prefix.chord.modifiers);
    if let Some(binding) = shortcuts
        .bindings
        .iter()
        .find(|b| match_chord(input_key, &mods_without_prefix, &b.chord))
    {
        let mux_ref = mux.bypass_change_detection();
        let mutated = crate::multiplexer::commands::apply(
            binding.action.clone(),
            mux_ref,
            attached.0.clone(),
        );
        if mutated {
            mux.set_changed();
        }
    }
    prefix.armed = false;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::configs::OzmuxConfigsResource;
    use crate::multiplexer::{AttachedSession, Multiplexer, OzmuxMultiplexerPlugin};
    use bevy::input::ButtonState;
    use bevy::input::keyboard::{Key as Bk, KeyboardInput, NativeKeyCode};
    use bevy::window::{Window, WindowResolution};
    use ozmux_configs::OzmuxConfigs;
    use ozmux_configs::shortcuts::{Key as CKey, Modifiers};

    fn make_app(window_focused: bool, armed: bool) -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(OzmuxMultiplexerPlugin)
            .add_systems(Update, dispatch_focused_key);
        app.insert_resource(ButtonInput::<KeyCode>::default());
        app.insert_resource(OzmuxConfigsResource(OzmuxConfigs::default()));
        app.add_message::<KeyboardInput>();

        let sid = {
            let mut mux = app.world_mut().resource_mut::<Multiplexer>();
            let sid = mux.create_session(Some("default".into()));
            mux.create_window(Some(&sid), Some("main".into())).unwrap();
            sid
        };
        let prefix_state = {
            let mut ps = PrefixState::from_prefix(
                &app.world()
                    .resource::<OzmuxConfigsResource>()
                    .shortcuts
                    .prefix,
            );
            ps.armed = armed;
            ps
        };
        let entity = app
            .world_mut()
            .spawn((
                Window {
                    focused: window_focused,
                    resolution: WindowResolution::new(800, 600),
                    ..default()
                },
                AttachedSession(sid),
                prefix_state,
            ))
            .id();
        (app, entity)
    }

    fn press(app: &mut App, window: Entity, key: Bk) {
        let ev = KeyboardInput {
            key_code: KeyCode::Unidentified(NativeKeyCode::Unidentified),
            logical_key: key,
            state: ButtonState::Pressed,
            text: None,
            repeat: false,
            window,
        };
        let mut events = app
            .world_mut()
            .resource_mut::<bevy::ecs::message::Messages<KeyboardInput>>();
        events.write(ev);
    }

    #[test]
    fn default_prefix_state_from_default_prefix_has_2s_timer() {
        let cfg = OzmuxConfigs::default();
        let p = PrefixState::from_prefix(&cfg.shortcuts.prefix);
        assert!(!p.armed);
        assert_eq!(p.timeout.duration().as_millis(), 2000);
        assert_eq!(p.timeout.mode(), TimerMode::Once);
    }

    #[test]
    fn match_chord_matches_char_with_no_modifiers() {
        let chord = KeyChord {
            key: CKey::Char('b'),
            modifiers: Modifiers::default(),
        };
        assert!(match_chord(&CKey::Char('b'), &Modifiers::default(), &chord));
        assert!(!match_chord(
            &CKey::Char('c'),
            &Modifiers::default(),
            &chord
        ));
    }

    #[test]
    fn match_chord_requires_matching_modifiers() {
        let chord = KeyChord {
            key: CKey::Char('c'),
            modifiers: Modifiers {
                shift: true,
                ..Default::default()
            },
        };
        assert!(match_chord(
            &CKey::Char('c'),
            &Modifiers {
                shift: true,
                ..Default::default()
            },
            &chord,
        ));
        assert!(!match_chord(
            &CKey::Char('c'),
            &Modifiers::default(),
            &chord,
        ));
    }

    #[test]
    fn bevy_to_configs_key_lowercases_ascii_alphabet() {
        assert_eq!(
            bevy_to_configs_key(&Bk::Character("S".into())),
            Some(CKey::Char('s'))
        );
        assert_eq!(
            bevy_to_configs_key(&Bk::Character("s".into())),
            Some(CKey::Char('s'))
        );
    }

    #[test]
    fn bevy_to_configs_key_preserves_symbols() {
        assert_eq!(
            bevy_to_configs_key(&Bk::Character("&".into())),
            Some(CKey::Char('&'))
        );
        assert_eq!(
            bevy_to_configs_key(&Bk::Character("{".into())),
            Some(CKey::Char('{'))
        );
    }

    #[test]
    fn bevy_to_configs_key_rejects_multichar_payload() {
        assert_eq!(bevy_to_configs_key(&Bk::Character("ab".into())), None);
    }

    #[test]
    fn bevy_to_configs_key_maps_named_keys() {
        assert_eq!(bevy_to_configs_key(&Bk::Escape), Some(CKey::Escape));
        assert_eq!(bevy_to_configs_key(&Bk::Enter), Some(CKey::Enter));
        assert_eq!(bevy_to_configs_key(&Bk::ArrowUp), Some(CKey::ArrowUp));
        assert_eq!(bevy_to_configs_key(&Bk::Tab), Some(CKey::Tab));
    }

    #[test]
    fn bevy_to_configs_key_returns_none_for_modifier_and_f_keys() {
        assert_eq!(bevy_to_configs_key(&Bk::Shift), None);
        assert_eq!(bevy_to_configs_key(&Bk::Control), None);
        assert_eq!(bevy_to_configs_key(&Bk::F1), None);
    }

    #[test]
    fn is_modifier_only_key_detects_held_modifiers_only() {
        assert!(is_modifier_only_key(&Bk::Shift));
        assert!(is_modifier_only_key(&Bk::Control));
        assert!(is_modifier_only_key(&Bk::Alt));
        assert!(is_modifier_only_key(&Bk::Super));
        assert!(is_modifier_only_key(&Bk::Meta));
        assert!(is_modifier_only_key(&Bk::Hyper));
        assert!(is_modifier_only_key(&Bk::AltGraph));
        assert!(is_modifier_only_key(&Bk::Fn));
        assert!(is_modifier_only_key(&Bk::Symbol));
        assert!(
            !is_modifier_only_key(&Bk::CapsLock),
            "CapsLock is a toggle press, not a held modifier — it must disarm"
        );
        assert!(!is_modifier_only_key(&Bk::NumLock));
        assert!(!is_modifier_only_key(&Bk::ScrollLock));
        assert!(!is_modifier_only_key(&Bk::FnLock));
        assert!(!is_modifier_only_key(&Bk::SymbolLock));
        assert!(!is_modifier_only_key(&Bk::Character("a".into())));
        assert!(!is_modifier_only_key(&Bk::F1));
    }

    #[test]
    fn ctrl_b_arms_prefix_on_focused_window() {
        let (mut app, entity) = make_app(true, false);
        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.press(KeyCode::ControlLeft);
        }
        press(&mut app, entity, Bk::Character("b".into()));
        app.update();
        let p = app.world().get::<PrefixState>(entity).unwrap();
        assert!(p.armed, "Ctrl-B must arm the prefix state");
        assert_eq!(
            app.world().resource::<Multiplexer>().sessions.len(),
            1,
            "arming alone must not change session count"
        );
    }

    #[test]
    fn ctrl_b_on_unfocused_window_does_not_arm() {
        let (mut app, entity) = make_app(false, false);
        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.press(KeyCode::ControlLeft);
        }
        press(&mut app, entity, Bk::Character("b".into()));
        app.update();
        assert!(!app.world().get::<PrefixState>(entity).unwrap().armed);
    }

    #[test]
    fn armed_then_c_fires_new_terminal_activity() {
        let (mut app, entity) = make_app(true, true);
        let activities_before = {
            let mux = app.world().resource::<Multiplexer>();
            let wid = mux.windows.keys().next().unwrap().clone();
            let window = mux.windows.get(&wid).unwrap();
            window
                .pane(&window.active_pane)
                .unwrap()
                .activity_ids()
                .count()
        };
        press(&mut app, entity, Bk::Character("c".into()));
        app.update();

        let mux = app.world().resource::<Multiplexer>();
        let wid = mux.windows.keys().next().unwrap().clone();
        let window = mux.windows.get(&wid).unwrap();
        let activities_after = window
            .pane(&window.active_pane)
            .unwrap()
            .activity_ids()
            .count();
        assert_eq!(activities_after, activities_before + 1);
        assert!(!app.world().get::<PrefixState>(entity).unwrap().armed);
    }

    #[test]
    fn armed_then_c_still_fires_when_ctrl_is_held_through_prefix() {
        let (mut app, entity) = make_app(true, true);
        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.press(KeyCode::ControlLeft);
        }
        let activities_before = {
            let mux = app.world().resource::<Multiplexer>();
            let wid = mux.windows.keys().next().unwrap().clone();
            let window = mux.windows.get(&wid).unwrap();
            window
                .pane(&window.active_pane)
                .unwrap()
                .activity_ids()
                .count()
        };
        press(&mut app, entity, Bk::Character("c".into()));
        app.update();

        let mux = app.world().resource::<Multiplexer>();
        let wid = mux.windows.keys().next().unwrap().clone();
        let window = mux.windows.get(&wid).unwrap();
        let activities_after = window
            .pane(&window.active_pane)
            .unwrap()
            .activity_ids()
            .count();
        assert_eq!(
            activities_after,
            activities_before + 1,
            "Ctrl held through Ctrl+B then C must still fire NewTerminalActivity"
        );
    }

    #[test]
    fn armed_then_shift_c_fires_new_window_via_uppercase_logical_key() {
        let (mut app, entity) = make_app(true, true);
        let windows_before = app.world().resource::<Multiplexer>().windows.len();

        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.press(KeyCode::ShiftLeft);
        }
        press(&mut app, entity, Bk::Character("C".into()));
        app.update();

        let mux = app.world().resource::<Multiplexer>();
        assert_eq!(
            mux.windows.len(),
            windows_before + 1,
            "Shift+C (logical 'C') must lowercase-match config 'c'+shift binding"
        );
    }

    #[test]
    fn armed_then_caps_lock_disarms() {
        let (mut app, entity) = make_app(true, true);
        press(&mut app, entity, Bk::CapsLock);
        app.update();
        assert!(
            !app.world().get::<PrefixState>(entity).unwrap().armed,
            "CapsLock is a toggle press, not a held modifier — it must disarm"
        );
    }

    #[test]
    fn armed_then_shift_alone_does_not_disarm() {
        let (mut app, entity) = make_app(true, true);
        press(&mut app, entity, Bk::Shift);
        app.update();
        assert!(
            app.world().get::<PrefixState>(entity).unwrap().armed,
            "modifier-only key must not disarm an armed prefix"
        );
    }

    #[test]
    fn armed_then_f1_disarms_without_firing_any_action() {
        let (mut app, entity) = make_app(true, true);
        let windows_before = app.world().resource::<Multiplexer>().windows.len();
        press(&mut app, entity, Bk::F1);
        app.update();
        assert!(
            !app.world().get::<PrefixState>(entity).unwrap().armed,
            "unmapped key (F1) must disarm"
        );
        assert_eq!(
            app.world().resource::<Multiplexer>().windows.len(),
            windows_before,
            "F1 must not fire any action"
        );
    }

    #[test]
    fn armed_then_unbound_lowercase_z_disarms_without_firing() {
        let (mut app, entity) = make_app(true, true);
        let windows_before = app.world().resource::<Multiplexer>().windows.len();
        press(&mut app, entity, Bk::Character("z".into()));
        app.update();
        assert!(!app.world().get::<PrefixState>(entity).unwrap().armed);
        assert_eq!(
            app.world().resource::<Multiplexer>().windows.len(),
            windows_before
        );
    }

    #[test]
    fn armed_then_x_closes_active_pane() {
        let (mut app, entity) = make_app(true, true);
        let sid = app
            .world()
            .resource::<Multiplexer>()
            .sessions
            .iter()
            .next()
            .map(|(id, _)| id)
            .unwrap()
            .clone();
        {
            let mut mux = app.world_mut().resource_mut::<Multiplexer>();
            crate::multiplexer::commands::apply(
                ozmux_configs::shortcuts::Action::SplitPane {
                    direction: ozmux_configs::shortcuts::SplitDirection::Horizontal,
                },
                mux.bypass_change_detection(),
                sid.clone(),
            );
        }
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
        assert_eq!(panes_before, 2);

        press(&mut app, entity, Bk::Character("x".into()));
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
            panes_before - 1,
            "armed Ctrl+B then x must close the active pane"
        );
        assert!(
            !app.world().get::<PrefixState>(entity).unwrap().armed,
            "dispatched chord must disarm the prefix state"
        );
    }

    #[test]
    fn armed_then_n_focuses_next_window() {
        let (mut app, entity) = make_app(true, true);
        let sid = app
            .world()
            .resource::<Multiplexer>()
            .sessions
            .iter()
            .next()
            .map(|(id, _)| id)
            .unwrap()
            .clone();
        {
            let mut mux = app.world_mut().resource_mut::<Multiplexer>();
            crate::multiplexer::commands::apply(
                ozmux_configs::shortcuts::Action::NewWindow,
                mux.bypass_change_detection(),
                sid.clone(),
            );
        }
        let linked_count_before = app
            .world()
            .resource::<Multiplexer>()
            .sessions
            .get(&sid)
            .unwrap()
            .linked_windows
            .len();
        assert_eq!(
            linked_count_before, 2,
            "setup must produce exactly 2 windows"
        );
        let active_before = app
            .world()
            .resource::<Multiplexer>()
            .sessions
            .get(&sid)
            .unwrap()
            .active_window
            .clone()
            .unwrap();

        press(&mut app, entity, Bk::Character("n".into()));
        app.update();

        let active_after = app
            .world()
            .resource::<Multiplexer>()
            .sessions
            .get(&sid)
            .unwrap()
            .active_window
            .clone()
            .unwrap();
        assert_ne!(
            active_after, active_before,
            "armed Ctrl+B then n must advance active_window"
        );
        assert!(
            !app.world().get::<PrefixState>(entity).unwrap().armed,
            "dispatched chord must disarm the prefix state"
        );
    }

    #[test]
    fn prefix_timeout_uses_config_value() {
        let mut cfg = OzmuxConfigs::default();
        cfg.shortcuts.prefix.timeout_ms = 1500;
        let p = PrefixState::from_prefix(&cfg.shortcuts.prefix);
        assert_eq!(p.timeout.duration().as_millis(), 1500);
    }

    #[test]
    fn shortcut_plugin_registers_systems_without_panic() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(OzmuxMultiplexerPlugin)
            .add_plugins(crate::configs::OzmuxConfigsPlugin)
            .add_plugins(OzmuxShortcutPlugin);
        app.insert_resource(ButtonInput::<KeyCode>::default());
        app.add_message::<KeyboardInput>();
        app.update();
    }
}
