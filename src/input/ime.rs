//! IME composition state for the terminal overlay.
//!
//! Provides `Composition` (a validated preedit snapshot), `ImeState`
//! (the active-composition resource), `read_ime_events` (the Bevy
//! system that drains `Ime` events and forwards `Ime::Commit` text to
//! the attached terminal), and `ime_policy_system` (toggles
//! `Window::ime_enabled` and `.ime_position`).

use crate::multiplexer::{AttachedSession, Multiplexer, SessionEntityId};
use crate::ui::TerminalActivityMarker;
use crate::ui::copy_mode::CopyModeState;
use crate::ui::registry::ActivityEntityRegistry;
use bevy::app::{App, Plugin, Update};
use bevy::ecs::message::MessageReader;
use bevy::ecs::query::With;
use bevy::ecs::resource::Resource;
use bevy::ecs::schedule::IntoScheduleConfigs;
use bevy::ecs::system::{Commands, Query, Res, ResMut};
use bevy::math::Vec2;
use bevy::ui::UiGlobalTransform;
use bevy::window::{Ime, PrimaryWindow, Window};
use bevy_terminal::{TerminalKey, TerminalModifiers};
use bevy_terminal_renderer::TerminalCellMetricsResource;
use bevy_terminal_renderer::prelude::TerminalGrid;

/// Bevy plugin that registers `ImeState` and the IME-event handling
/// systems. Ordering: `ime_policy_system` runs before
/// `read_ime_events`, both run before `dispatch_focused_key` (the
/// `.after(read_ime_events)` constraint on `dispatch_focused_key` is
/// added in `OzmuxShortcutPlugin`).
pub struct ImePlugin;

impl Plugin for ImePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ImeState>()
            .add_systems(Update, (ime_policy_system, read_ime_events).chain());
    }
}

/// Validated snapshot of a preedit string and its UTF-8-safe caret
/// position.
#[derive(Debug)]
pub(crate) struct Composition {
    text: String,
    caret: Option<usize>,
}

impl Composition {
    /// Validates and constructs a `Composition`. Returns `None` when:
    ///   - `text` is empty (treat any empty-value Preedit as
    ///     "no composition");
    ///   - `raw_caret.0` is out of bounds or lands on a non-UTF-8
    ///     boundary byte (defensive: winit returns byte offsets that
    ///     we later slice into).
    ///
    /// Only honors `raw_caret.0` (the begin offset); the selection
    /// range is out of scope per the design spec, Decision 3.
    pub(crate) fn try_new(text: String, raw_caret: Option<(usize, usize)>) -> Option<Self> {
        if text.is_empty() {
            return None;
        }
        let caret = match raw_caret {
            None => None,
            Some((begin, _end)) => {
                if begin <= text.len() && text.is_char_boundary(begin) {
                    Some(begin)
                } else {
                    None
                }
            }
        };
        Some(Composition { text, caret })
    }

    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    pub(crate) fn caret(&self) -> Option<usize> {
        self.caret
    }
}

/// IME composition state for the primary window.
///
/// `None` = no active preedit (overlay hidden, key dispatch normal).
/// `Some(_)` = a non-empty preedit is showing and key dispatch is
/// suppressed.
///
/// The window's `ime_enabled` field is the single source of truth for
/// whether IME is allowed; this resource intentionally does not mirror
/// it.
#[derive(Resource, Default, Debug)]
pub(crate) struct ImeState(Option<Composition>);

impl ImeState {
    pub(crate) fn is_composing(&self) -> bool {
        self.0.is_some()
    }

    pub(crate) fn composition(&self) -> Option<&Composition> {
        self.0.as_ref()
    }
}

/// Pure-function state machine: applies one `Ime` event to `state` and
/// returns the text that should be committed to the PTY (only set on
/// `Ime::Commit`).
///
/// Keeping this pure makes the state transitions unit-testable without
/// a Bevy `App` harness; the Bevy system in `read_ime_events` is a thin
/// wrapper around this.
pub(crate) fn apply_event(state: &mut ImeState, event: &Ime) -> Option<String> {
    match event {
        Ime::Enabled { .. } => None,
        Ime::Disabled { .. } => {
            state.0 = None;
            None
        }
        Ime::Preedit { value, cursor, .. } => {
            state.0 = Composition::try_new(value.clone(), *cursor);
            None
        }
        Ime::Commit { value, .. } => {
            state.0 = None;
            Some(value.clone())
        }
    }
}

/// Derives whether IME should be on this tick and writes
/// `PrimaryWindow.ime_enabled` and `.ime_position`.
///
/// `ime_enabled` is `true` iff (a) the attached activity carries
/// `TerminalActivityMarker`, and (b) it does NOT have `CopyModeState`.
///
/// `ime_position` is the logical-pixel anchor for the OS candidate
/// window — computed from the attached terminal's `UiGlobalTransform`
/// translation + `TerminalGrid.cursor` × cell pitch, then divided by
/// the window scale factor.
pub(crate) fn ime_policy_system(
    attached_sid_q: Query<&SessionEntityId, With<AttachedSession>>,
    mux: Res<Multiplexer>,
    registry: Res<ActivityEntityRegistry>,
    terminal_q: Query<(), With<TerminalActivityMarker>>,
    copy_mode_q: Query<(), With<CopyModeState>>,
    anchor_q: Query<(&UiGlobalTransform, &TerminalGrid)>,
    metrics: Res<TerminalCellMetricsResource>,
    mut window_q: Query<&mut Window, With<PrimaryWindow>>,
) {
    let Ok(mut window) = window_q.single_mut() else {
        return;
    };

    let Some(entity) = super::resolve_focused_terminal(&attached_sid_q, &mux, &registry) else {
        if window.ime_enabled {
            window.ime_enabled = false;
        }
        return;
    };

    let is_terminal = terminal_q.get(entity).is_ok();
    let in_copy_mode = copy_mode_q.get(entity).is_ok();
    let desired = is_terminal && !in_copy_mode;

    if window.ime_enabled != desired {
        window.ime_enabled = desired;
    }

    if !desired {
        return;
    }

    // NOTE: Anchor `ime_position` at the cursor cell origin (no
    // row-below offset — that offset is only used for the inline
    // overlay; the OS candidate window has its own placement logic
    // relative to this point).
    let Ok((ui_xform, grid)) = anchor_q.get(entity) else {
        return;
    };
    let scale = window.resolution.scale_factor();
    let cell_w_phys = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h_phys = metrics.metrics.line_height_phys.floor().max(1.0);
    let cursor_cell = grid.cursor.clone().unwrap_or_default();
    let host_origin_phys = ui_xform.translation * scale;
    let cell_origin_phys = host_origin_phys
        + Vec2::new(
            cursor_cell.x as f32 * cell_w_phys,
            cursor_cell.y as f32 * cell_h_phys,
        );
    let pos_logical = cell_origin_phys / scale;
    if window.ime_position != pos_logical {
        window.ime_position = pos_logical;
    }
}

/// Drains `Ime` events, mutates `ImeState`, and forwards `Ime::Commit`
/// text to the attached terminal.
///
/// Modifiers are forced to `TerminalModifiers::default()` on commit:
/// `crates/bevy_terminal/src/input_codec.rs::encode_key` converts
/// `Text("a")` to control byte `0x01` when `ctrl` is held, which would
/// silently corrupt a single-ASCII-letter IME commit (e.g., the
/// macOS Character Viewer emoji path).
pub(crate) fn read_ime_events(
    mut events: MessageReader<Ime>,
    mut state: ResMut<ImeState>,
    mut commands: Commands,
    attached_sid_q: Query<&SessionEntityId, With<AttachedSession>>,
    mux: Res<Multiplexer>,
    registry: Res<ActivityEntityRegistry>,
) {
    for event in events.read() {
        if let Some(commit_text) = apply_event(&mut state, event) {
            let Some(sid) = attached_sid_q.iter().next().map(|s| s.0) else {
                tracing::warn!(
                    target: "ozmux_gui::input::ime",
                    "commit dropped: no attached terminal",
                );
                continue;
            };
            super::forward_to_active_terminal(
                &mut commands,
                &mux,
                &registry,
                &sid,
                TerminalKey::Text(commit_text),
                TerminalModifiers::default(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multiplexer::{
        AttachedSession, Multiplexer, OzmuxMultiplexerPlugin, SessionEntityId,
    };
    use crate::ui::registry::ActivityEntityRegistry;
    use bevy::app::{App, Update};
    use bevy::ecs::entity::Entity;
    use bevy::ecs::observer::On;
    use bevy::ecs::resource::Resource;
    use bevy::prelude::{MinimalPlugins, default};
    use bevy::window::{Ime, Window, WindowResolution};
    use bevy_terminal::{TerminalKey, TerminalKeyInput, TerminalModifiers};
    use std::sync::{Arc, Mutex};

    #[test]
    fn try_new_returns_none_for_empty_text() {
        assert!(Composition::try_new(String::new(), None).is_none());
        assert!(Composition::try_new(String::new(), Some((0, 0))).is_none());
    }

    #[test]
    fn try_new_accepts_valid_caret() {
        let c = Composition::try_new("hello".into(), Some((3, 3))).unwrap();
        assert_eq!(c.text(), "hello");
        assert_eq!(c.caret(), Some(3));
    }

    #[test]
    fn try_new_accepts_caret_at_text_len() {
        let c = Composition::try_new("ab".into(), Some((2, 2))).unwrap();
        assert_eq!(c.caret(), Some(2));
    }

    #[test]
    fn try_new_clamps_out_of_bounds_caret_to_none() {
        let c = Composition::try_new("ab".into(), Some((99, 99))).unwrap();
        assert_eq!(c.text(), "ab");
        assert_eq!(c.caret(), None);
    }

    #[test]
    fn try_new_rejects_non_char_boundary_caret() {
        // "あ" is 3 bytes in UTF-8; byte 1 is mid-char.
        let c = Composition::try_new("あ".into(), Some((1, 1))).unwrap();
        assert_eq!(c.text(), "あ");
        assert_eq!(c.caret(), None);
    }

    #[test]
    fn try_new_honors_only_begin_offset() {
        // The end offset is ignored per Decision 3 (no selection range).
        let c = Composition::try_new("hello".into(), Some((2, 5))).unwrap();
        assert_eq!(c.caret(), Some(2));
    }

    #[test]
    fn try_new_with_none_caret_keeps_none() {
        let c = Composition::try_new("hi".into(), None).unwrap();
        assert_eq!(c.caret(), None);
    }

    fn dummy_window() -> Entity {
        Entity::from_bits(1)
    }

    #[test]
    fn apply_enabled_is_noop() {
        let mut s = ImeState::default();
        let out = apply_event(
            &mut s,
            &Ime::Enabled {
                window: dummy_window(),
            },
        );
        assert!(out.is_none());
        assert!(!s.is_composing());
    }

    #[test]
    fn apply_nonempty_preedit_sets_composition() {
        let mut s = ImeState::default();
        let event = Ime::Preedit {
            window: dummy_window(),
            value: "こんに".into(),
            cursor: Some((3, 3)),
        };
        let out = apply_event(&mut s, &event);
        assert!(out.is_none());
        let c = s.composition().expect("composition set");
        assert_eq!(c.text(), "こんに");
        assert_eq!(c.caret(), Some(3));
    }

    #[test]
    fn apply_empty_preedit_clears_composition() {
        let mut s = ImeState::default();
        apply_event(
            &mut s,
            &Ime::Preedit {
                window: dummy_window(),
                value: "ab".into(),
                cursor: Some((1, 1)),
            },
        );
        assert!(s.is_composing());

        apply_event(
            &mut s,
            &Ime::Preedit {
                window: dummy_window(),
                value: String::new(),
                cursor: None,
            },
        );
        assert!(!s.is_composing());
    }

    #[test]
    fn apply_disabled_clears_composition() {
        let mut s = ImeState::default();
        apply_event(
            &mut s,
            &Ime::Preedit {
                window: dummy_window(),
                value: "ab".into(),
                cursor: Some((1, 1)),
            },
        );
        apply_event(
            &mut s,
            &Ime::Disabled {
                window: dummy_window(),
            },
        );
        assert!(!s.is_composing());
    }

    #[test]
    fn apply_commit_returns_text_and_clears_composition() {
        let mut s = ImeState::default();
        apply_event(
            &mut s,
            &Ime::Preedit {
                window: dummy_window(),
                value: "ab".into(),
                cursor: Some((1, 1)),
            },
        );
        let out = apply_event(
            &mut s,
            &Ime::Commit {
                window: dummy_window(),
                value: "こんにちは".into(),
            },
        );
        assert_eq!(out.as_deref(), Some("こんにちは"));
        assert!(!s.is_composing());
    }

    #[test]
    fn apply_cursor_none_preedit_clears_caret() {
        let mut s = ImeState::default();
        apply_event(
            &mut s,
            &Ime::Preedit {
                window: dummy_window(),
                value: "ab".into(),
                cursor: None,
            },
        );
        let c = s.composition().unwrap();
        assert_eq!(c.text(), "ab");
        assert_eq!(c.caret(), None);
    }

    // Integration harness for read_ime_events app-driven tests.
    // Mirrors the pattern from src/input.rs::tests::make_app / install_active_terminal_activity.
    // We do NOT use ImePlugin because ime_policy_system requires TerminalCellMetricsResource
    // and TerminalGrid, which are unavailable in the minimal test environment.

    #[derive(Resource, Default, Clone)]
    struct CapturedKeys(Arc<Mutex<Vec<TerminalKeyInput>>>);

    fn capture_key_input(ev: On<TerminalKeyInput>, captured: Res<CapturedKeys>) {
        captured.0.lock().unwrap().push((*ev).clone());
    }

    fn build_app_with_attached_entity() -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(OzmuxMultiplexerPlugin)
            .add_systems(Update, read_ime_events);
        app.init_resource::<ImeState>();
        app.init_resource::<ActivityEntityRegistry>();
        app.insert_resource(CapturedKeys::default());
        app.add_observer(capture_key_input);
        // Ime is a Message; register its channel so MessageReader<Ime> is available.
        app.add_message::<Ime>();

        // Create a session in the multiplexer and spawn the attached session entity.
        let sid = {
            let mut mux = app.world_mut().resource_mut::<Multiplexer>();
            let (sid, _, _) = mux.create_session(Some("default".into()));
            sid
        };
        app.world_mut()
            .spawn((SessionEntityId(sid), AttachedSession));

        // Spawn a terminal activity entity and register it in the registry so
        // resolve_focused_terminal / forward_to_active_terminal resolve to it.
        let term_entity = app.world_mut().spawn_empty().id();
        let activity_id = {
            let mux = app.world().resource::<Multiplexer>();
            let session = mux.sessions.get(&sid).unwrap();
            let pane = session.pane(&session.active_pane).unwrap();
            pane.active_activity.clone()
        };
        {
            let mut registry = app.world_mut().resource_mut::<ActivityEntityRegistry>();
            registry.insert_for_test(activity_id, term_entity);
        }

        // Spawn a focused window so any dispatch path that queries Window works.
        app.world_mut().spawn(Window {
            focused: true,
            resolution: WindowResolution::new(800, 600),
            ..default()
        });

        (app, term_entity)
    }

    #[test]
    fn commit_forwards_with_default_modifiers() {
        let (mut app, term_entity) = build_app_with_attached_entity();

        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<Ime>>()
            .write(Ime::Commit {
                window: Entity::PLACEHOLDER,
                value: "こんにちは".into(),
            });

        app.update();

        let captured = app
            .world()
            .resource::<CapturedKeys>()
            .0
            .lock()
            .unwrap()
            .clone();
        assert_eq!(captured.len(), 1, "expected exactly one TerminalKeyInput");
        assert_eq!(captured[0].entity, term_entity);
        assert!(
            matches!(&captured[0].key, TerminalKey::Text(s) if s == "こんにちは"),
            "key payload mismatch: {:?}",
            captured[0].key,
        );
        assert_eq!(
            captured[0].modifiers,
            TerminalModifiers::default(),
            "modifiers MUST be default — see input_codec.rs::encode_key ctrl path",
        );
    }

    #[test]
    fn commit_dropped_when_no_attached_terminal() {
        let (mut app, _term_entity) = build_app_with_attached_entity();
        let attached: Vec<Entity> = app
            .world_mut()
            .query_filtered::<Entity, With<AttachedSession>>()
            .iter(app.world())
            .collect();
        for e in attached {
            app.world_mut().despawn(e);
        }

        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<Ime>>()
            .write(Ime::Commit {
                window: Entity::PLACEHOLDER,
                value: "x".into(),
            });

        app.update();

        let captured = app
            .world()
            .resource::<CapturedKeys>()
            .0
            .lock()
            .unwrap()
            .clone();
        assert!(
            captured.is_empty(),
            "commit should be dropped when no AttachedSession"
        );
    }
}
