//! IME composition state for the terminal overlay.
//!
//! Provides `Composition` (a validated preedit snapshot), `ImeState`
//! (the active-composition resource), and `read_ime_events` (the Bevy
//! system that drains `Ime` events and forwards `Ime::Commit` text to
//! the attached terminal). The `ime_policy_system` that toggles
//! `Window::ime_enabled` is added in a later task.

use bevy::ecs::message::MessageReader;
use bevy::ecs::query::With;
use bevy::ecs::resource::Resource;
use bevy::ecs::system::{Commands, Query, Res, ResMut};
use bevy::window::Ime;
use bevy_terminal::{TerminalKey, TerminalModifiers};
use crate::multiplexer::{AttachedSession, Multiplexer, SessionEntityId};
use crate::ui::registry::ActivityEntityRegistry;

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
    use bevy::ecs::entity::Entity;
    use bevy::window::Ime;

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
        let out = apply_event(&mut s, &Ime::Enabled { window: dummy_window() });
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
        apply_event(&mut s, &Ime::Disabled { window: dummy_window() });
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
}
