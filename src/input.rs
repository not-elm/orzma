//! Keyboard shortcut handling: dispatcher systems. The shortcut binding
//! table comes from the loaded `OzmuxConfigsResource`; this module owns
//! no chord data.

pub(crate) mod ime;
pub(crate) mod mouse_buttons;
pub(crate) mod mouse_wheel;

use crate::input::ime::{ImeState, read_ime_events};
use crate::multiplexer::{AttachedSession, Multiplexer, SessionEntityId};
use crate::ui::registry::ActivityEntityRegistry;
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use bevy_terminal::{TerminalKey, TerminalKeyInput, TerminalModifiers};
use ozmux_configs::shortcuts::{Action, KeyChord, Modifiers, SessionOffset};
use ozmux_multiplexer::SessionId;
use std::collections::HashSet;

/// Resolves the focused activity's entity via the attached session →
/// multiplexer → registry chain.
pub(crate) fn resolve_focused_terminal(
    attached_sid_q: &Query<&SessionEntityId, With<AttachedSession>>,
    mux: &Multiplexer,
    registry: &ActivityEntityRegistry,
) -> Option<Entity> {
    let attached = attached_sid_q.iter().next()?;
    let session = mux.sessions.get(&attached.0)?;
    let pane = session.pane(&session.active_pane).ok()?;
    registry.get(&pane.active_activity)
}

/// Bevy Plugin that registers the keyboard shortcut handling pipeline.
pub struct OzmuxShortcutPlugin;

impl Plugin for OzmuxShortcutPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            dispatch_focused_key
                .in_set(crate::system_set::OzmuxSystems::Input)
                .after(read_ime_events),
        );
    }
}

pub(crate) fn dispatch_focused_key(
    mut commands: Commands,
    mut events: MessageReader<KeyboardInput>,
    mut mux: ResMut<Multiplexer>,
    mut clipboard: ResMut<crate::clipboard::Clipboard>,
    mut handles: Query<(
        &mut bevy_terminal::TerminalHandle,
        &mut bevy_terminal::PtyHandle,
        &mut bevy_terminal::Coalescer,
    )>,
    windows_q: Query<&Window>,
    sessions_q: Query<(Entity, &SessionEntityId)>,
    attached_q: Query<Entity, With<AttachedSession>>,
    attached_sid_q: Query<&SessionEntityId, With<AttachedSession>>,
    keys: Res<ButtonInput<KeyCode>>,
    configs: Res<crate::configs::OzmuxConfigsResource>,
    registry: Res<crate::ui::registry::ActivityEntityRegistry>,
    copy_mode_q: Query<(), With<crate::ui::copy_mode::CopyModeState>>,
    ime_state: Res<ImeState>,
) {
    let bindings = &configs.shortcuts.bindings;
    // NOTE: ButtonInput<KeyCode> is updated in PreUpdate; every Update-tick event
    // sees the same modifier snapshot. Read once outside the loop.
    let mods = current_modifiers(&keys);
    let mut just_exited: HashSet<Entity> = HashSet::new();
    // NOTE: Bevy command-flush race guard. dispatch_new_session and
    // dispatch_focus_session queue Commands that mutate the AttachedSession
    // marker; the deferred flush only runs after this system returns. If two
    // marker-mutating actions fire in the same Update tick (e.g., user
    // double-taps Cmd+R), both would read the same pre-flush attached entity,
    // resulting in zero or multiple AttachedSession entities after flush —
    // breaking the `exactly one attached` invariant relied on by
    // attached_sid_q.single() and downstream rebuild systems. Drop the
    // second-and-onward marker mutations in this frame.
    let mut marker_dirty_this_frame = false;

    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }

        if ime_state.is_composing() {
            continue;
        }

        let Ok(win) = windows_q.get(ev.window) else {
            continue;
        };
        if !win.focused {
            continue;
        }

        let attached_sid = match attached_sid_q.single() {
            Ok(sid) => sid,
            Err(err) => {
                // NOTE: silently dropping keystrokes here would be invisible to
                // the user. The invariant 'exactly one entity carries
                // AttachedSession' is enforced by bootstrap + dispatch_new_session
                // + dispatch_focus_session; if it's violated we want a loud
                // signal in the log so the failure mode is observable.
                tracing::warn!(
                    target: "ozmux_gui::input",
                    ?err,
                    "attached_sid_q.single() failed; dropping keystroke (AttachedSession invariant violated)"
                );
                continue;
            }
        };
        let session_id = attached_sid.0;

        if matches!(ev.logical_key, Key::Escape)
            && let Some(session) = mux.sessions.get(&session_id)
            && let Ok(pane) = session.pane(&session.active_pane)
            && let Some(entity) = registry.get(&pane.active_activity)
            && copy_mode_q.get(entity).is_err()
            && let Ok((mut handle, _pty, mut coalescer)) = handles.get_mut(entity)
            && !handle.is_at_bottom()
        {
            handle.scroll_to_bottom(&mut coalescer);
            continue;
        }

        if is_copy_chord(&ev.logical_key, &mods)
            && let Some(session) = mux.sessions.get(&session_id)
            && let Ok(pane) = session.pane(&session.active_pane)
            && let Some(entity) = registry.get(&pane.active_activity)
            && let Ok((handle, _pty, _coalescer)) = handles.get_mut(entity)
            && let Some(text) = handle.selection_to_string()
            && !text.is_empty()
        {
            clipboard.write(text);
            continue;
        }

        if let Some(session) = mux.sessions.get(&session_id)
            && let Ok(pane) = session.pane(&session.active_pane)
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
            if let Some(session) = mux.sessions.get(&session_id)
                && let Ok(pane) = session.pane(&session.active_pane)
                && let Some(entity) = registry.get(&pane.active_activity)
                && let Ok((mut handle, mut pty, _coalescer)) = handles.get_mut(entity)
                && let Some(text) = clipboard.read()
                && !text.is_empty()
            {
                let bracketed = handle.bracketed_paste_enabled();
                let bytes = crate::clipboard::build_paste_bytes(&text, bracketed);
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
                // OS key-repeat suppression: only block shortcut actions, not terminal input.
                if ev.repeat {
                    continue;
                }
                execute_action(
                    action,
                    &mut commands,
                    &mut mux,
                    session_id,
                    &sessions_q,
                    &attached_q,
                    &registry,
                    &mut marker_dirty_this_frame,
                );
                continue;
            }
        }

        if let Some(tk) = bevy_to_terminal_key(&ev.logical_key) {
            forward_to_active_terminal(
                &mut commands,
                &mux,
                &registry,
                &session_id,
                tk,
                shortcut_mods_to_terminal_mods(&mods),
            );
        }
    }
}

pub(crate) fn current_modifiers(keys: &ButtonInput<KeyCode>) -> Modifiers {
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

/// Returns `true` when the (key, mods) pair is the "copy active
/// selection to clipboard" shortcut `Cmd+C`. The match is strict —
/// exactly `meta` is held; `ctrl` / `shift` / `alt` are absent. The
/// `c` character match is case-sensitive (uppercase `C` does not bind,
/// since `Cmd+Shift+C` is not the copy shortcut on macOS).
///
/// Lives as a fast-path predicate rather than a `Bindings` action for
/// the same reason `is_paste_chord` does — sit symmetric with paste,
/// and avoid touching the binding-table schema for a non-rebindable
/// chord. See spec §8.
fn is_copy_chord(key: &Key, mods: &Modifiers) -> bool {
    let Key::Character(s) = key else { return false };
    if s.as_str() != "c" {
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
/// mutating the multiplexer. `Action::NewSession`,
/// `Action::FocusSession`, and `Action::FocusSessionNumber` are
/// dispatched directly in Bevy-land because they require entity-level
/// side effects beyond a pure `MultiplexerService` mutation.
fn execute_action(
    action: Action,
    commands: &mut Commands,
    mux: &mut ResMut<Multiplexer>,
    session_id: SessionId,
    sessions_q: &Query<(Entity, &SessionEntityId)>,
    attached_q: &Query<Entity, With<AttachedSession>>,
    registry: &ActivityEntityRegistry,
    marker_dirty_this_frame: &mut bool,
) {
    match &action {
        Action::EnterCopyMode => {
            if let Some(session) = mux.sessions.get(&session_id)
                && let Ok(pane) = session.pane(&session.active_pane)
                && let Some(entity) = registry.get(&pane.active_activity)
            {
                commands.trigger(crate::ui::copy_mode::EnterCopyModeRequest { entity });
            }
        }
        Action::NewSession => {
            if *marker_dirty_this_frame {
                tracing::warn!(
                    target: "ozmux_gui::input",
                    "skipping NewSession: AttachedSession marker already mutated this frame (would race the deferred Bevy command flush)"
                );
                return;
            }
            *marker_dirty_this_frame = true;
            dispatch_new_session(commands, mux, attached_q);
        }
        Action::FocusSession { .. } | Action::FocusSessionNumber { .. } => {
            if *marker_dirty_this_frame {
                tracing::warn!(
                    target: "ozmux_gui::input",
                    ?action,
                    "skipping FocusSession*: AttachedSession marker already mutated this frame (would race the deferred Bevy command flush)"
                );
                return;
            }
            *marker_dirty_this_frame = true;
            dispatch_focus_session(commands, mux, sessions_q, attached_q, &action);
        }
        _ => {
            let mux_ref = mux.bypass_change_detection();
            let mutated = crate::multiplexer::commands::apply(action, mux_ref, session_id);
            if mutated {
                mux.bump_epoch(&session_id);
                mux.set_changed();
            }
        }
    }
}

/// Moves the `AttachedSession` marker between session entities for the
/// `FocusSession{Next,Prev}` and `FocusSessionNumber{index}` actions.
/// Sorts session entities numerically by `SessionEntityId` for deterministic
/// cycle order. Bumps `Multiplexer::set_changed()` so the UI rebuild system
/// (which polls on `mux.is_changed()`) fires.
fn dispatch_focus_session(
    commands: &mut Commands,
    mux: &mut ResMut<Multiplexer>,
    sessions_q: &Query<(Entity, &SessionEntityId)>,
    attached_q: &Query<Entity, With<AttachedSession>>,
    action: &Action,
) {
    let mut entries: Vec<(SessionEntityId, Entity)> =
        sessions_q.iter().map(|(e, sid)| (*sid, e)).collect();
    if entries.len() < 2 {
        return;
    }
    entries.sort_by_key(|(sid, _)| *sid);

    let Ok(current_entity) = attached_q.single() else {
        return;
    };
    let current_idx = entries
        .iter()
        .position(|(_, e)| *e == current_entity)
        .unwrap_or(0);

    let target_idx = match action {
        Action::FocusSession {
            offset: SessionOffset::Next,
        } => (current_idx + 1) % entries.len(),
        Action::FocusSession {
            offset: SessionOffset::Prev,
        } => current_idx.checked_sub(1).unwrap_or(entries.len() - 1),
        Action::FocusSession {
            offset: SessionOffset::Last,
        } => {
            tracing::debug!(
                target: "ozmux_gui::commands",
                "FocusSession::Last not yet implemented"
            );
            return;
        }
        Action::FocusSessionNumber { index } => {
            let i = *index as usize;
            if i >= entries.len() {
                return;
            }
            i
        }
        _ => return,
    };

    let target_entity = entries[target_idx].1;
    if target_entity == current_entity {
        return;
    }

    commands.entity(current_entity).remove::<AttachedSession>();
    commands.entity(target_entity).insert(AttachedSession);
    // NOTE: focus-session moves the AttachedSession marker only; no per-session
    // domain state changes, so we do not bump any session epoch. The rebuild
    // system intentionally ignores this signal; sync_active_session picks it up.
    mux.set_changed();
}

/// Mints a new domain session, spawns its Bevy entity with the
/// `AttachedSession` marker, and removes the marker from the previously
/// attached entity. Bumps Multiplexer change detection so the UI rebuild
/// fires this frame.
fn dispatch_new_session(
    commands: &mut Commands,
    mux: &mut ResMut<Multiplexer>,
    attached_q: &Query<Entity, With<AttachedSession>>,
) {
    let (sid, _, _) = mux.create_session(None);

    if let Ok(previous_attached) = attached_q.single() {
        commands
            .entity(previous_attached)
            .remove::<AttachedSession>();
    }

    let bevy_name = mux
        .sessions
        .get(&sid)
        .map(|s| s.name.clone())
        .unwrap_or_else(|| format!("Session#{}", sid.0));
    let subtree_root = commands
        .spawn(Node {
            width: bevy::ui::Val::Percent(100.0),
            height: bevy::ui::Val::Percent(100.0),
            ..default()
        })
        .id();
    let new_session_entity = commands
        .spawn((
            SessionEntityId(sid),
            AttachedSession,
            crate::multiplexer::SessionUiSubtree(subtree_root),
            Name::new(bevy_name),
        ))
        .id();
    commands
        .entity(subtree_root)
        .insert(ChildOf(new_session_entity));

    mux.bump_epoch(&sid);
    mux.set_changed();
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
/// active pane/activity yet, or when the target entity has no
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
    let Some(session) = mux.sessions.get(sid) else {
        return;
    };
    let Ok(pane) = session.pane(&session.active_pane) else {
        return;
    };
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
    use crate::multiplexer::{
        AttachedSession, Multiplexer, OzmuxMultiplexerPlugin, SessionEntityId,
    };
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
        app.init_resource::<crate::input::ime::ImeState>();
        app.insert_resource(crate::clipboard::Clipboard::new());
        app.add_message::<KeyboardInput>();

        let sid = {
            let mut mux = app.world_mut().resource_mut::<Multiplexer>();
            let (sid, _, _) = mux.create_session(Some("default".into()));
            sid
        };
        // Spawn the session entity carrying the AttachedSession marker. This
        // mirrors what `bootstrap` does in production. The window entity is
        // spawned separately (without AttachedSession).
        app.world_mut()
            .spawn((SessionEntityId(sid), AttachedSession));
        let window_entity = app
            .world_mut()
            .spawn(Window {
                focused: window_focused,
                resolution: WindowResolution::new(800, 600),
                ..default()
            })
            .id();
        (app, window_entity)
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
            let sid = *mux.sessions.keys().next().unwrap();
            let session = mux.sessions.get(&sid).unwrap();
            let pane = session.pane(&session.active_pane).unwrap();
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
            let sid = *mux.sessions.keys().next().unwrap();
            let session = mux.sessions.get(&sid).unwrap();
            let pane = session.pane(&session.active_pane).unwrap();
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
    fn is_copy_chord_matches_meta_c_only() {
        assert!(super::is_copy_chord(
            &Bk::Character("c".into()),
            &Modifiers {
                meta: true,
                ..Default::default()
            },
        ));
    }

    #[test]
    fn is_copy_chord_rejects_meta_shift_c() {
        assert!(!super::is_copy_chord(
            &Bk::Character("c".into()),
            &Modifiers {
                meta: true,
                shift: true,
                ..Default::default()
            },
        ));
    }

    #[test]
    fn is_copy_chord_rejects_plain_c() {
        assert!(!super::is_copy_chord(
            &Bk::Character("c".into()),
            &Modifiers::default(),
        ));
    }

    #[test]
    fn is_copy_chord_rejects_ctrl_c() {
        assert!(!super::is_copy_chord(
            &Bk::Character("c".into()),
            &Modifiers {
                ctrl: true,
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
        app.init_resource::<crate::input::ime::ImeState>();
        app.insert_resource(crate::clipboard::Clipboard::new());
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
                .resource_mut::<crate::clipboard::Clipboard>();
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
            let cb = app.world().resource::<crate::clipboard::Clipboard>();
            cb.is_available_for_test()
        };
        if !clipboard_available {
            eprintln!("skipping: arboard unavailable in this environment (e.g. headless CI)");
            return;
        }
        {
            let mut clipboard = app
                .world_mut()
                .resource_mut::<crate::clipboard::Clipboard>();
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

    #[test]
    fn direct_dispatch_cmd_j_fires_focus_pane_down() {
        let (mut app, window_entity) = make_app(true);
        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.press(KeyCode::SuperLeft);
        }
        press(&mut app, window_entity, Bk::Character("j".into()));
        app.update();
        let mux = app.world().resource::<crate::multiplexer::Multiplexer>();
        assert!(!mux.sessions.is_empty());
    }

    #[test]
    fn key_repeat_event_is_ignored() {
        let (mut app, window_entity) = make_app(true);
        install_active_terminal_activity(&mut app);
        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.press(KeyCode::SuperLeft);
        }
        let ev = KeyboardInput {
            key_code: KeyCode::Unidentified(NativeKeyCode::Unidentified),
            logical_key: Bk::Character("d".into()),
            state: ButtonState::Pressed,
            text: None,
            repeat: true,
            window: window_entity,
        };
        let mut events = app
            .world_mut()
            .resource_mut::<bevy::ecs::message::Messages<KeyboardInput>>();
        events.write(ev);
        drop(events);
        app.update();
        let mux = app.world().resource::<crate::multiplexer::Multiplexer>();
        assert!(!mux.sessions.is_empty());
    }

    #[test]
    fn unbound_chord_falls_through_to_terminal_passthrough() {
        let (mut app, window_entity) = make_app(true);
        app.insert_resource(CapturedKeys::default());
        app.add_observer(capture_key_input);
        install_active_terminal_activity(&mut app);
        press(&mut app, window_entity, Bk::Character("a".into()));
        app.update();
        let captured = app.world().resource::<CapturedKeys>().0.lock().unwrap();
        assert!(
            captured.iter().any(|ev| matches!(
                &ev.key,
                bevy_terminal::TerminalKey::Text(s) if s == "a"
            )),
            "plain 'a' must forward to the terminal; captured: {:?}",
            captured,
        );
    }

    #[test]
    fn key_repeat_event_forwards_to_terminal() {
        let (mut app, window_entity) = make_app(true);
        app.insert_resource(CapturedKeys::default());
        app.add_observer(capture_key_input);
        install_active_terminal_activity(&mut app);
        let ev = KeyboardInput {
            key_code: KeyCode::Unidentified(NativeKeyCode::Unidentified),
            logical_key: Bk::Character("j".into()),
            state: ButtonState::Pressed,
            text: None,
            repeat: true,
            window: window_entity,
        };
        let mut events = app
            .world_mut()
            .resource_mut::<bevy::ecs::message::Messages<KeyboardInput>>();
        events.write(ev);
        drop(events);
        app.update();
        let captured = app.world().resource::<CapturedKeys>().0.lock().unwrap();
        assert!(
            captured.iter().any(|ev| matches!(
                &ev.key,
                bevy_terminal::TerminalKey::Text(s) if s == "j"
            )),
            "repeat=true 'j' must still forward to the terminal; captured: {:?}",
            captured,
        );
    }

    fn attached_session_id(app: &mut App) -> SessionEntityId {
        let world = app.world_mut();
        let mut q = world.query_filtered::<&SessionEntityId, With<AttachedSession>>();
        *q.single(world)
            .expect("exactly one attached session entity")
    }

    fn count_attached_session_entities(app: &mut App) -> usize {
        let world = app.world_mut();
        let mut q = world.query_filtered::<Entity, With<AttachedSession>>();
        q.iter(world).count()
    }

    fn count_session_entities(app: &mut App) -> usize {
        let world = app.world_mut();
        let mut q = world.query::<&SessionEntityId>();
        q.iter(world).count()
    }

    #[test]
    fn focus_session_next_moves_attached_marker() {
        let (mut app, _window_entity) = make_app(true);

        // Mint a second session via the domain API and spawn its entity
        // (without AttachedSession, the bootstrap entity keeps the marker).
        let second_sid = {
            let mut mux = app.world_mut().resource_mut::<Multiplexer>();
            let (sid, _, _) = mux.create_session(Some("second".into()));
            sid
        };
        let second_entity = app.world_mut().spawn(SessionEntityId(second_sid)).id();

        // Sanity: bootstrap session (SessionId 0) is attached, second is not.
        assert_eq!(count_attached_session_entities(&mut app), 1);
        assert_eq!(
            attached_session_id(&mut app).0,
            ozmux_multiplexer::SessionId(0)
        );

        // Use a one-shot system that invokes the dispatcher.
        let id = app.world_mut().register_system(
            |mut commands: Commands,
             mut mux: ResMut<Multiplexer>,
             sessions_q: Query<(Entity, &SessionEntityId)>,
             attached_q: Query<Entity, With<AttachedSession>>| {
                super::dispatch_focus_session(
                    &mut commands,
                    &mut mux,
                    &sessions_q,
                    &attached_q,
                    &ozmux_configs::shortcuts::Action::FocusSession {
                        offset: ozmux_configs::shortcuts::SessionOffset::Next,
                    },
                );
            },
        );
        let _ = app.world_mut().run_system(id);
        app.update();

        // Marker moved to the second entity.
        assert_eq!(count_attached_session_entities(&mut app), 1);
        let attached_now = attached_session_id(&mut app);
        assert_eq!(attached_now.0, second_sid);
        assert!(
            app.world().get::<AttachedSession>(second_entity).is_some(),
            "second entity must now carry the marker"
        );
    }

    #[test]
    fn focus_session_number_targets_index() {
        let (mut app, _window_entity) = make_app(true);

        let second_sid = {
            let mut mux = app.world_mut().resource_mut::<Multiplexer>();
            let (sid, _, _) = mux.create_session(Some("second".into()));
            sid
        };
        app.world_mut().spawn(SessionEntityId(second_sid));

        let id = app.world_mut().register_system(
            |mut commands: Commands,
             mut mux: ResMut<Multiplexer>,
             sessions_q: Query<(Entity, &SessionEntityId)>,
             attached_q: Query<Entity, With<AttachedSession>>| {
                super::dispatch_focus_session(
                    &mut commands,
                    &mut mux,
                    &sessions_q,
                    &attached_q,
                    &ozmux_configs::shortcuts::Action::FocusSessionNumber { index: 1 },
                );
            },
        );
        let _ = app.world_mut().run_system(id);
        app.update();

        assert_eq!(attached_session_id(&mut app).0, second_sid);
    }

    #[test]
    fn dispatch_new_session_spawns_subtree_pointer() {
        use crate::multiplexer::{
            AttachedSession, Multiplexer, OzmuxMultiplexerPlugin, SessionEntityId, SessionUiSubtree,
        };
        use bevy::ecs::system::RunSystemOnce;

        let _guard = crate::configs::env_guard();
        // SAFETY: env mutations are serialized by env_guard() for this crate's tests.
        unsafe {
            std::env::remove_var("OZMUX_CONFIG");
        }

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(OzmuxMultiplexerPlugin)
            .add_plugins(crate::configs::OzmuxConfigsPlugin)
            .add_plugins(crate::bootstrap::OzmuxBootstrapPlugin);
        app.update();

        let session_count_before = {
            let world = app.world_mut();
            let mut q = world.query_filtered::<Entity, With<SessionEntityId>>();
            q.iter(world).count()
        };

        app.world_mut()
            .run_system_once(
                |mut commands: Commands,
                 mut mux: ResMut<Multiplexer>,
                 attached_q: Query<Entity, With<AttachedSession>>| {
                    super::dispatch_new_session(&mut commands, &mut mux, &attached_q);
                },
            )
            .unwrap();
        app.update();

        let world = app.world_mut();
        let mut q = world.query::<(&SessionEntityId, &SessionUiSubtree)>();
        let count = q.iter(world).count();
        assert_eq!(
            count,
            session_count_before + 1,
            "new session entity must carry SessionUiSubtree"
        );
    }

    #[test]
    fn new_session_action_spawns_entity_and_moves_marker() {
        let (mut app, _window_entity) = make_app(true);

        assert_eq!(count_session_entities(&mut app), 1);
        assert_eq!(count_attached_session_entities(&mut app), 1);
        let bootstrap_sid = attached_session_id(&mut app).0;

        let id = app.world_mut().register_system(
            |mut commands: Commands,
             mut mux: ResMut<Multiplexer>,
             attached_q: Query<Entity, With<AttachedSession>>| {
                super::dispatch_new_session(&mut commands, &mut mux, &attached_q);
            },
        );
        let _ = app.world_mut().run_system(id);
        app.update();

        assert_eq!(count_session_entities(&mut app), 2);
        assert_eq!(count_attached_session_entities(&mut app), 1);
        let new_attached_sid = attached_session_id(&mut app).0;
        assert_ne!(new_attached_sid, bootstrap_sid);
        let mux = app.world().resource::<Multiplexer>();
        assert_eq!(mux.sessions.len(), 2);
    }

    /// Two `NewSession` actions in a single Update tick must NOT both queue
    /// `commands.spawn((..., AttachedSession))`. Without the
    /// `marker_dirty_this_frame` guard, both invocations would observe the
    /// same pre-flush attached entity, each queue a fresh spawn-with-marker,
    /// and after the deferred command flush there would be two entities
    /// carrying `AttachedSession` — breaking `single()` for every downstream
    /// system and silently freezing keyboard input.
    #[test]
    fn two_new_session_actions_in_one_frame_keep_marker_invariant() {
        let (mut app, _window_entity) = make_app(true);

        assert_eq!(count_session_entities(&mut app), 1);
        assert_eq!(count_attached_session_entities(&mut app), 1);

        // Simulate the dispatch loop calling execute_action twice in one tick
        // by registering a system that invokes it twice with the same shared
        // `marker_dirty_this_frame` flag.
        let id = app.world_mut().register_system(
            |mut commands: Commands,
             mut mux: ResMut<Multiplexer>,
             sessions_q: Query<(Entity, &SessionEntityId)>,
             attached_q: Query<Entity, With<AttachedSession>>,
             registry: Res<crate::ui::registry::ActivityEntityRegistry>| {
                let mut marker_dirty = false;
                let dummy_session = ozmux_multiplexer::SessionId(0);
                super::execute_action(
                    ozmux_configs::shortcuts::Action::NewSession,
                    &mut commands,
                    &mut mux,
                    dummy_session,
                    &sessions_q,
                    &attached_q,
                    &registry,
                    &mut marker_dirty,
                );
                super::execute_action(
                    ozmux_configs::shortcuts::Action::NewSession,
                    &mut commands,
                    &mut mux,
                    dummy_session,
                    &sessions_q,
                    &attached_q,
                    &registry,
                    &mut marker_dirty,
                );
            },
        );
        let _ = app.world_mut().run_system(id);
        app.update();

        // Exactly one new session entity spawned, exactly one attached marker.
        assert_eq!(
            count_attached_session_entities(&mut app),
            1,
            "AttachedSession marker invariant preserved across same-frame double NewSession"
        );
        assert_eq!(
            count_session_entities(&mut app),
            2,
            "second NewSession in same frame must NOT spawn a third entity"
        );
        let mux = app.world().resource::<Multiplexer>();
        assert_eq!(
            mux.sessions.len(),
            2,
            "domain Multiplexer also reflects only one extra session"
        );
    }

    #[test]
    fn dispatch_focused_key_suppressed_during_composition() {
        let (mut app, window_entity) = make_app(true);
        app.insert_resource(CapturedKeys::default());
        app.add_observer(capture_key_input);
        install_active_terminal_activity(&mut app);

        // Drive ImeState into composing mode directly via apply_event.
        {
            let mut state = app
                .world_mut()
                .resource_mut::<crate::input::ime::ImeState>();
            crate::input::ime::apply_event(
                &mut state,
                &bevy::window::Ime::Preedit {
                    window: Entity::PLACEHOLDER,
                    value: "あ".into(),
                    cursor: Some((3, 3)),
                },
            );
        }

        press(&mut app, window_entity, Bk::Character("a".into()));
        app.update();

        let captured = app.world().resource::<CapturedKeys>().0.lock().unwrap();
        assert!(
            captured.is_empty(),
            "dispatch_focused_key must suppress keys while ImeState is composing; captured: {:?}",
            captured,
        );
    }
}
