//! Keyboard shortcut handling: dispatcher systems. The shortcut binding
//! table comes from the loaded `OzmuxConfigsResource`; this module owns
//! no chord data.

pub(crate) mod mouse_wheel;

use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use bevy_terminal::{TerminalKey, TerminalKeyInput, TerminalModifiers};
use ozmux_configs::shortcuts::{Action, KeyChord, Modifiers};
use ozmux_multiplexer::SessionId;
use std::collections::HashSet;

/// Bevy Plugin that registers the keyboard shortcut handling pipeline.
pub struct OzmuxShortcutPlugin;

impl Plugin for OzmuxShortcutPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, dispatch_focused_key);
    }
}

pub(crate) fn dispatch_focused_key(
    mut commands: Commands,
    mut events: MessageReader<KeyboardInput>,
    keys: Res<ButtonInput<KeyCode>>,
    configs: Res<crate::configs::OzmuxConfigsResource>,
    registry: Res<crate::ui::registry::ActivityEntityRegistry>,
    mut mux: ResMut<crate::multiplexer::Multiplexer>,
    copy_mode_q: Query<(), With<crate::ui::copy_mode::CopyModeState>>,
    mut clipboard: ResMut<crate::ui::clipboard::Clipboard>,
    mut handles: Query<(
        &mut bevy_terminal::TerminalHandle,
        &mut bevy_terminal::PtyHandle,
        &mut bevy_terminal::Coalescer,
    )>,
    mut q: Query<(&crate::multiplexer::AttachedSession, &Window)>,
) {
    let bindings = &configs.shortcuts.bindings;
    // NOTE: ButtonInput<KeyCode> is updated in PreUpdate; every Update-tick event
    // sees the same modifier snapshot. Read once outside the loop.
    let mods = current_modifiers(&keys);
    let mut just_exited: HashSet<Entity> = HashSet::new();

    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        if ev.repeat {
            continue;
        }

        let Ok((attached, win)) = q.get_mut(ev.window) else {
            continue;
        };
        if !win.focused {
            continue;
        }

        if matches!(ev.logical_key, Key::Escape)
            && let Ok((wid, pid)) = mux.active_pane_of_session(&attached.0)
            && let Some(window) = mux.windows.get(&wid)
            && let Ok(pane) = window.pane(&pid)
            && let Some(entity) = registry.get(&pane.active_activity)
            && copy_mode_q.get(entity).is_err()
            && let Ok((mut handle, _pty, mut coalescer)) = handles.get_mut(entity)
            && !handle.is_at_bottom()
        {
            handle.scroll_to_bottom(&mut coalescer);
            continue;
        }

        if let Ok((wid, pid)) = mux.active_pane_of_session(&attached.0)
            && let Some(window) = mux.windows.get(&wid)
            && let Ok(pane) = window.pane(&pid)
            && let Some(entity) = registry.get(&pane.active_activity)
            && copy_mode_q.get(entity).is_ok()
            && !just_exited.contains(&entity)
        {
            let exited = crate::ui::copy_mode::dispatch_key(
                &mut commands,
                &mut handles,
                &mut clipboard,
                entity,
                ev.logical_key.clone(),
                mods.clone(),
            );
            if exited {
                just_exited.insert(entity);
            }
            continue;
        }

        if is_paste_chord(&ev.logical_key, &mods) {
            if let Ok((wid, pid)) = mux.active_pane_of_session(&attached.0)
                && let Some(window) = mux.windows.get(&wid)
                && let Ok(pane) = window.pane(&pid)
                && let Some(entity) = registry.get(&pane.active_activity)
                && let Ok((mut handle, mut pty, _coalescer)) = handles.get_mut(entity)
                && let Some(text) = clipboard.read()
                && !text.is_empty()
            {
                let bracketed = handle.bracketed_paste_enabled();
                let bytes = crate::ui::clipboard::build_paste_bytes(&text, bracketed);
                if let Err(err) = handle.write(&mut pty, &bytes) {
                    tracing::warn!(
                        target: "ozmux_gui::input",
                        ?err,
                        "paste PTY write failed",
                    );
                }
            }
            continue;
        }

        if is_modifier_only_key(&ev.logical_key) {
            continue;
        }

        if let Some(input_key) = bevy_to_configs_key(&ev.logical_key) {
            let chord = KeyChord {
                key: input_key,
                modifiers: mods.clone(),
            };
            if let Some(action) = bindings.lookup(&chord) {
                execute_action(action, &mut commands, &mut mux, attached, &registry);
                continue;
            }
        }

        if let Some(tk) = bevy_to_terminal_key(&ev.logical_key) {
            forward_to_active_terminal(
                &mut commands,
                &mux,
                &registry,
                &attached.0,
                tk,
                shortcut_mods_to_terminal_mods(&mods),
            );
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

fn is_modifier_only_key(key: &Key) -> bool {
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

/// Returns `true` when the (key, mods) pair is the OS paste shortcut
/// `Cmd+V`. The match is strict on modifiers — exactly `meta` is held,
/// and `ctrl` / `shift` / `alt` are all absent. The `v` character match
/// is case-sensitive (uppercase `V` does not bind, since Shift+Cmd+V
/// is not a paste shortcut on macOS).
fn is_paste_chord(key: &Key, mods: &Modifiers) -> bool {
    let Key::Character(s) = key else { return false };
    if s.as_str() != "v" {
        return false;
    }
    mods.meta && !mods.ctrl && !mods.shift && !mods.alt
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
            if c.is_ascii_alphabetic() {
                return Some(CKey::Char(c.to_ascii_lowercase()));
            }
            // NOTE: Symbol+Shift reverse-normalize on US ASCII layout. macOS
            // reports the shifted glyph (e.g., '{' when Shift+'[' is pressed),
            // but our bindings target the unshifted glyph and carry Shift in
            // modifiers. Without this map, `Cmd+Shift+[` defaults never match.
            match c {
                '{' => CKey::Char('['),
                '}' => CKey::Char(']'),
                '<' => CKey::Char(','),
                '>' => CKey::Char('.'),
                '?' => CKey::Char('/'),
                ':' => CKey::Char(';'),
                '"' => CKey::Char('\''),
                '|' => CKey::Char('\\'),
                '~' => CKey::Char('`'),
                '_' => CKey::Char('-'),
                '+' => CKey::Plus,
                '!' => CKey::Char('1'),
                '@' => CKey::Char('2'),
                '#' => CKey::Char('3'),
                '$' => CKey::Char('4'),
                '%' => CKey::Char('5'),
                '^' => CKey::Char('6'),
                '&' => CKey::Char('7'),
                '*' => CKey::Char('8'),
                '(' => CKey::Char('9'),
                ')' => CKey::Char('0'),
                _ => CKey::Char(c),
            }
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

/// Executes a resolved `Action` against the multiplexer.
///
/// Preserves the existing `bypass_change_detection()` + selective
/// `set_changed()` discipline so that ECS change detection only fires
/// when a real domain mutation happens. `Action::EnterCopyMode` is
/// handled specially because it triggers an observer rather than
/// mutating the multiplexer.
fn execute_action(
    action: Action,
    commands: &mut Commands,
    mux: &mut ResMut<crate::multiplexer::Multiplexer>,
    attached: &crate::multiplexer::AttachedSession,
    registry: &crate::ui::registry::ActivityEntityRegistry,
) {
    if let Action::EnterCopyMode = action {
        if let Ok((wid, pid)) = mux.active_pane_of_session(&attached.0)
            && let Some(window) = mux.windows.get(&wid)
            && let Ok(pane) = window.pane(&pid)
            && let Some(entity) = registry.get(&pane.active_activity)
        {
            commands.trigger(crate::ui::copy_mode::EnterCopyModeRequest { entity });
        }
        return;
    }
    let mux_ref = mux.bypass_change_detection();
    let mutated = crate::multiplexer::commands::apply(action, mux_ref, attached.0.clone());
    if mutated {
        mux.set_changed();
    }
}

/// Translates a Bevy logical key into the `TerminalKey` variant the
/// `bevy_terminal` codec accepts. Returns `None` for keys the terminal
/// does not consume (F-keys, modifier-only keys, etc. — those keys are
/// silently dropped).
fn bevy_to_terminal_key(key: &Key) -> Option<TerminalKey> {
    Some(match key {
        Key::Character(s) => TerminalKey::Text(s.to_string()),
        Key::Space => TerminalKey::Text(" ".into()),
        Key::Enter => TerminalKey::Enter,
        Key::Backspace => TerminalKey::Backspace,
        Key::Tab => TerminalKey::Tab,
        Key::Escape => TerminalKey::Escape,
        Key::Delete => TerminalKey::Delete,
        Key::ArrowUp => TerminalKey::ArrowUp,
        Key::ArrowDown => TerminalKey::ArrowDown,
        Key::ArrowLeft => TerminalKey::ArrowLeft,
        Key::ArrowRight => TerminalKey::ArrowRight,
        Key::Home => TerminalKey::Home,
        Key::End => TerminalKey::End,
        Key::PageUp => TerminalKey::PageUp,
        Key::PageDown => TerminalKey::PageDown,
        _ => return None,
    })
}

/// Converts shortcut-layer `Modifiers` into the `TerminalModifiers` carried
/// on the `TerminalKeyInput` EntityEvent. MVP only reads `ctrl` on the
/// receiving side; the other fields are forwarded for future use.
fn shortcut_mods_to_terminal_mods(m: &Modifiers) -> TerminalModifiers {
    TerminalModifiers {
        ctrl: m.ctrl,
        shift: m.shift,
        alt: m.alt,
        meta: m.meta,
    }
}

/// Resolves the active activity entity for `sid` and triggers a
/// `TerminalKeyInput` on it. Silently no-ops when the session has no
/// active window/pane/activity yet, or when the target entity has no
/// `TerminalHandle` (e.g. Browser Activity) — the `bevy_terminal`
/// observer handles that case by also no-op'ing.
fn forward_to_active_terminal(
    commands: &mut Commands,
    mux: &crate::multiplexer::Multiplexer,
    registry: &crate::ui::registry::ActivityEntityRegistry,
    sid: &SessionId,
    key: TerminalKey,
    mods: TerminalModifiers,
) {
    let Ok((wid, pid)) = mux.active_pane_of_session(sid) else {
        return;
    };
    let Some(window) = mux.windows.get(&wid) else {
        return;
    };
    let Ok(pane) = window.pane(&pid) else { return };
    let Some(entity) = registry.get(&pane.active_activity) else {
        return;
    };
    commands.trigger(TerminalKeyInput {
        entity,
        key,
        modifiers: mods,
    });
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

    fn make_app(window_focused: bool) -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(OzmuxMultiplexerPlugin)
            .add_systems(Update, dispatch_focused_key);
        app.insert_resource(ButtonInput::<KeyCode>::default());
        app.insert_resource(OzmuxConfigsResource(OzmuxConfigs::default()));
        app.init_resource::<crate::ui::registry::ActivityEntityRegistry>();
        app.insert_resource(crate::ui::clipboard::Clipboard::new());
        app.add_message::<KeyboardInput>();

        let sid = {
            let mut mux = app.world_mut().resource_mut::<Multiplexer>();
            let sid = mux.create_session(Some("default".into()));
            mux.create_window(Some(&sid), Some("main".into())).unwrap();
            sid
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

    use bevy::ecs::observer::On;
    use bevy_terminal::TerminalKeyInput;
    use std::sync::{Arc, Mutex};

    #[derive(Resource, Default, Clone)]
    struct CapturedKeys(Arc<Mutex<Vec<TerminalKeyInput>>>);

    fn capture_key_input(ev: On<TerminalKeyInput>, captured: Res<CapturedKeys>) {
        captured.0.lock().unwrap().push((*ev).clone());
    }

    /// Spawns a registry-registered Terminal Activity entity inside the
    /// active pane of the only window in the test app, returning its Entity id.
    /// The entity carries NO `TerminalHandle`, so the `bevy_terminal`
    /// observer no-ops on the missing component — the test capture
    /// observer still records the trigger regardless of observer order.
    fn install_active_terminal_activity(app: &mut App) -> Entity {
        let entity = app.world_mut().spawn_empty().id();
        let activity_id = {
            let mux = app.world().resource::<Multiplexer>();
            let wid = mux.windows.keys().next().unwrap().clone();
            let window = mux.windows.get(&wid).unwrap();
            let pane = window.pane(&window.active_pane).unwrap();
            pane.active_activity.clone()
        };
        let mut registry = app
            .world_mut()
            .resource_mut::<crate::ui::registry::ActivityEntityRegistry>();
        registry.insert_for_test(activity_id, entity);
        entity
    }

    /// Spawns a registry-registered Terminal Activity entity that carries
    /// a real `TerminalHandle` / `PtyHandle` / `Coalescer` (via
    /// `TerminalBundle::spawn`). Used by the paste-gate integration tests
    /// that need to observe `pending_user_input` flipping after the gate
    /// runs.
    fn install_active_terminal_activity_with_handle(app: &mut App) -> Entity {
        let opts = bevy_terminal::SpawnOptions {
            cols: 10,
            rows: 5,
            shell: "/bin/sh".into(),
            cwd: None,
            env: Vec::new(),
        };
        let bundle = bevy_terminal::TerminalBundle::spawn(opts).expect("spawn /bin/sh");
        let entity = app.world_mut().spawn(bundle).id();
        let activity_id = {
            let mux = app.world().resource::<Multiplexer>();
            let wid = mux.windows.keys().next().unwrap().clone();
            let window = mux.windows.get(&wid).unwrap();
            let pane = window.pane(&window.active_pane).unwrap();
            pane.active_activity.clone()
        };
        let mut registry = app
            .world_mut()
            .resource_mut::<crate::ui::registry::ActivityEntityRegistry>();
        registry.insert_for_test(activity_id, entity);
        entity
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
    fn bevy_to_configs_key_normalizes_shift_symbols() {
        assert_eq!(
            bevy_to_configs_key(&Bk::Character("&".into())),
            Some(CKey::Char('7'))
        );
        assert_eq!(
            bevy_to_configs_key(&Bk::Character("{".into())),
            Some(CKey::Char('['))
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
    fn bevy_to_configs_key_normalizes_shifted_left_bracket() {
        use ozmux_configs::shortcuts::Key as CKey;
        let k = bevy_to_configs_key(&bevy::input::keyboard::Key::Character("{".into()));
        assert_eq!(k, Some(CKey::Char('[')));
    }

    #[test]
    fn bevy_to_configs_key_normalizes_shifted_right_bracket() {
        use ozmux_configs::shortcuts::Key as CKey;
        let k = bevy_to_configs_key(&bevy::input::keyboard::Key::Character("}".into()));
        assert_eq!(k, Some(CKey::Char(']')));
    }

    #[test]
    fn bevy_to_configs_key_maps_plus_character_to_key_plus() {
        use ozmux_configs::shortcuts::Key as CKey;
        let k = bevy_to_configs_key(&bevy::input::keyboard::Key::Character("+".into()));
        assert_eq!(k, Some(CKey::Plus));
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
            "CapsLock is a toggle press, not a held modifier"
        );
        assert!(!is_modifier_only_key(&Bk::NumLock));
        assert!(!is_modifier_only_key(&Bk::ScrollLock));
        assert!(!is_modifier_only_key(&Bk::FnLock));
        assert!(!is_modifier_only_key(&Bk::SymbolLock));
        assert!(!is_modifier_only_key(&Bk::Character("a".into())));
        assert!(!is_modifier_only_key(&Bk::F1));
    }

    #[test]
    fn is_paste_chord_matches_meta_v_only() {
        assert!(super::is_paste_chord(
            &Bk::Character("v".into()),
            &Modifiers {
                meta: true,
                ..Default::default()
            },
        ));
    }

    #[test]
    fn is_paste_chord_rejects_plain_v() {
        assert!(!super::is_paste_chord(
            &Bk::Character("v".into()),
            &Modifiers::default(),
        ));
    }

    #[test]
    fn is_paste_chord_rejects_meta_plus_extra_modifier() {
        assert!(!super::is_paste_chord(
            &Bk::Character("v".into()),
            &Modifiers {
                meta: true,
                ctrl: true,
                ..Default::default()
            },
        ));
        assert!(!super::is_paste_chord(
            &Bk::Character("v".into()),
            &Modifiers {
                meta: true,
                shift: true,
                ..Default::default()
            },
        ));
        assert!(!super::is_paste_chord(
            &Bk::Character("v".into()),
            &Modifiers {
                meta: true,
                alt: true,
                ..Default::default()
            },
        ));
    }

    #[test]
    fn is_paste_chord_rejects_uppercase_v() {
        assert!(!super::is_paste_chord(
            &Bk::Character("V".into()),
            &Modifiers {
                meta: true,
                ..Default::default()
            },
        ));
    }

    #[test]
    fn is_paste_chord_rejects_other_keys() {
        assert!(!super::is_paste_chord(
            &Bk::Character("c".into()),
            &Modifiers {
                meta: true,
                ..Default::default()
            },
        ));
        assert!(!super::is_paste_chord(
            &Bk::Escape,
            &Modifiers {
                meta: true,
                ..Default::default()
            },
        ));
    }

    #[test]
    fn shortcut_plugin_registers_systems_without_panic() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(OzmuxMultiplexerPlugin)
            .add_plugins(crate::configs::OzmuxConfigsPlugin)
            .add_plugins(OzmuxShortcutPlugin);
        app.insert_resource(ButtonInput::<KeyCode>::default());
        app.init_resource::<crate::ui::registry::ActivityEntityRegistry>();
        app.insert_resource(crate::ui::clipboard::Clipboard::new());
        app.add_message::<KeyboardInput>();
        app.update();
    }

    #[test]
    fn unbound_key_forwards_to_terminal() {
        let (mut app, window_entity) = make_app(true);
        app.insert_resource(CapturedKeys::default());
        app.add_observer(capture_key_input);
        let _activity_entity = install_active_terminal_activity(&mut app);

        press(&mut app, window_entity, Bk::Character("h".into()));
        app.update();

        let captured = app.world().resource::<CapturedKeys>().0.lock().unwrap();
        assert_eq!(
            captured.len(),
            1,
            "single 'h' must produce exactly one TerminalKeyInput"
        );
        assert!(
            matches!(&captured[0].key, bevy_terminal::TerminalKey::Text(s) if s == "h"),
            "captured key was {:?}",
            captured[0].key
        );
    }

    #[test]
    fn enter_forwards_as_terminal_enter() {
        let (mut app, window_entity) = make_app(true);
        app.insert_resource(CapturedKeys::default());
        app.add_observer(capture_key_input);
        install_active_terminal_activity(&mut app);

        press(&mut app, window_entity, Bk::Enter);
        app.update();

        let captured = app.world().resource::<CapturedKeys>().0.lock().unwrap();
        assert_eq!(captured.len(), 1);
        assert!(matches!(captured[0].key, bevy_terminal::TerminalKey::Enter));
    }

    #[test]
    fn no_active_terminal_entity_means_no_panic_just_silent_drop() {
        let (mut app, window_entity) = make_app(true);
        app.insert_resource(CapturedKeys::default());
        app.add_observer(capture_key_input);

        press(&mut app, window_entity, Bk::Character("h".into()));
        app.update();

        let captured = app.world().resource::<CapturedKeys>().0.lock().unwrap();
        assert!(captured.is_empty(), "no registry entry → no trigger");
    }

    #[test]
    fn key_consumed_by_copy_mode_gate_does_not_reach_terminal() {
        let (mut app, window_entity) = make_app(true);
        app.insert_resource(CapturedKeys::default());
        app.add_observer(capture_key_input);
        let activity_entity = install_active_terminal_activity(&mut app);
        app.world_mut()
            .entity_mut(activity_entity)
            .insert(crate::ui::copy_mode::CopyModeState);

        press(&mut app, window_entity, Bk::Character("h".into()));
        app.update();

        let captured = app.world().resource::<CapturedKeys>().0.lock().unwrap();
        assert!(
            captured.is_empty(),
            "active CopyModeState must consume keys before terminal-forward (captured: {:?})",
            captured,
        );
    }

    #[test]
    fn keys_after_y_in_same_frame_reach_terminal_not_copy_mode() {
        let (mut app, window_entity) = make_app(true);
        app.insert_resource(CapturedKeys::default());
        app.add_observer(capture_key_input);
        let activity_entity = install_active_terminal_activity(&mut app);
        app.world_mut()
            .entity_mut(activity_entity)
            .insert(crate::ui::copy_mode::CopyModeState);

        press(&mut app, window_entity, Bk::Character("y".into()));
        press(&mut app, window_entity, Bk::Character("a".into()));
        app.update();

        let captured = app.world().resource::<CapturedKeys>().0.lock().unwrap();
        assert_eq!(
            captured.len(),
            1,
            "expected exactly one TerminalKeyInput (for 'a' after 'y' exited copy mode), captured: {:?}",
            captured,
        );
        assert!(
            matches!(&captured[0].key, bevy_terminal::TerminalKey::Text(s) if s == "a"),
            "captured key was {:?}, expected Text(\"a\")",
            captured[0].key,
        );
    }

    #[test]
    fn cmd_v_with_bracketed_paste_off_writes_to_pty_and_consumes_key() {
        let (mut app, window_entity) = make_app(true);
        app.insert_resource(CapturedKeys::default());
        app.add_observer(capture_key_input);
        let _activity_entity = install_active_terminal_activity_with_handle(&mut app);
        {
            let mut clipboard = app
                .world_mut()
                .resource_mut::<crate::ui::clipboard::Clipboard>();
            clipboard.write("hello\nworld".to_string());
        }

        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.press(KeyCode::SuperLeft);
        }
        press(&mut app, window_entity, Bk::Character("v".into()));
        app.update();

        let captured = app.world().resource::<CapturedKeys>().0.lock().unwrap();
        assert!(
            captured.is_empty(),
            "Cmd+V must NOT forward as TerminalKeyInput; captured: {:?}",
            captured,
        );
    }

    #[test]
    fn cmd_v_with_bracketed_paste_on_wraps_bytes_when_clipboard_has_text() {
        let (mut app, window_entity) = make_app(true);
        let activity_entity = install_active_terminal_activity_with_handle(&mut app);
        let clipboard_available = {
            let cb = app.world().resource::<crate::ui::clipboard::Clipboard>();
            cb.is_available_for_test()
        };
        if !clipboard_available {
            eprintln!("skipping: arboard unavailable in this environment (e.g. headless CI)");
            return;
        }
        {
            let mut clipboard = app
                .world_mut()
                .resource_mut::<crate::ui::clipboard::Clipboard>();
            clipboard.write("hi".to_string());
        }
        {
            let mut handle = app
                .world_mut()
                .get_mut::<bevy_terminal::TerminalHandle>(activity_entity)
                .unwrap();
            handle.advance(b"\x1b[?2004h");
        }
        assert!(
            !app.world()
                .get::<bevy_terminal::TerminalHandle>(activity_entity)
                .unwrap()
                .pending_user_input(),
            "precondition: no pending input before paste",
        );

        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.press(KeyCode::SuperLeft);
        }
        press(&mut app, window_entity, Bk::Character("v".into()));
        app.update();

        assert!(
            app.world()
                .get::<bevy_terminal::TerminalHandle>(activity_entity)
                .unwrap()
                .pending_user_input(),
            "after Cmd+V with bracketed paste on and seeded clipboard, handle.write must have been called (flipping pending_user_input to true)",
        );
    }

    #[test]
    fn cmd_v_in_copy_mode_does_not_invoke_paste() {
        let (mut app, window_entity) = make_app(true);
        app.insert_resource(CapturedKeys::default());
        app.add_observer(capture_key_input);
        let activity_entity = install_active_terminal_activity_with_handle(&mut app);
        app.world_mut()
            .entity_mut(activity_entity)
            .insert(crate::ui::copy_mode::CopyModeState);
        assert!(
            !app.world()
                .get::<bevy_terminal::TerminalHandle>(activity_entity)
                .unwrap()
                .pending_user_input()
        );

        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.press(KeyCode::SuperLeft);
        }
        press(&mut app, window_entity, Bk::Character("v".into()));
        app.update();

        assert!(
            !app.world()
                .get::<bevy_terminal::TerminalHandle>(activity_entity)
                .unwrap()
                .pending_user_input(),
            "copy-mode gate must consume Cmd+V before the paste gate; no write should occur",
        );
        let captured = app.world().resource::<CapturedKeys>().0.lock().unwrap();
        assert!(
            captured.is_empty(),
            "Cmd+V in copy mode must not leak to the terminal",
        );
    }

    #[test]
    fn cmd_v_then_next_key_reaches_terminal() {
        let (mut app, window_entity) = make_app(true);
        app.insert_resource(CapturedKeys::default());
        app.add_observer(capture_key_input);
        install_active_terminal_activity_with_handle(&mut app);

        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.press(KeyCode::SuperLeft);
        }
        press(&mut app, window_entity, Bk::Character("v".into()));
        app.update();

        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.release(KeyCode::SuperLeft);
        }
        press(&mut app, window_entity, Bk::Character("a".into()));
        app.update();

        let captured = app.world().resource::<CapturedKeys>().0.lock().unwrap();
        assert!(
            captured.iter().any(|ev| matches!(
                &ev.key,
                bevy_terminal::TerminalKey::Text(s) if s == "a"
            )),
            "after Cmd+V, the next plain 'a' must forward to the terminal; captured: {:?}",
            captured,
        );
    }
}
