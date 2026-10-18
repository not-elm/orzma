//! IME composition state for the terminal overlay: drains IME events into
//! `ImeState` and derives window IME policy from keyboard focus.

mod ime_target;
#[cfg(test)]
mod test_support;

use crate::action::vi::mode::ViModeState;
use crate::input::InputPhase;
use crate::input::focus::KeyboardFocused;
use crate::surface::OrzmaTerminal;
use crate::ui::tab_bar::rename::{RenameSystems, TabRename};
use bevy::app::{App, Last, MainScheduleOrder, Plugin, PostUpdate, Update};
use bevy::ecs::change_detection::DetectChangesMut;
use bevy::ecs::entity::Entity;
use bevy::ecs::event::EntityEvent;
use bevy::ecs::message::MessageReader;
use bevy::ecs::observer::On;
use bevy::ecs::query::With;
use bevy::ecs::resource::Resource;
use bevy::ecs::schedule::common_conditions::{on_message, resource_exists_and_changed};
use bevy::ecs::schedule::{IntoScheduleConfigs, ScheduleLabel, SystemCondition};
use bevy::ecs::system::{Commands, NonSendMarker, Query, Res, ResMut};
use bevy::math::Rect;
use bevy::window::{Ime, PrimaryWindow, Window, WindowBackendScaleFactorChanged};
use bevy::winit::WINIT_WINDOWS;
use bevy_cef::prelude::FocusedWebview;
use bevy_orzmux::prelude::RequestActiveKeyInput;
use ime_target::{ImePolicyParams, ImeTarget};
use orzma_tty::prelude::{KeyText, TerminalKey, TerminalModifiers};
use winit::dpi::{LogicalPosition, LogicalSize};

/// IME-committed text destined for the keyboard-focused terminal surface.
///
/// Applying it writes the committed text to the active pane.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct ImeCommit {
    #[event_target]
    pub(crate) entity: Entity,
    pub(crate) text: String,
}

/// Adds `ImeState`, IME event handling, and the window IME policy.
pub(super) struct ImePlugin;

impl Plugin for ImePlugin {
    fn build(&self, app: &mut App) {
        app.world_mut()
            .resource_mut::<MainScheduleOrder>()
            .insert_after(Last, ImeCandidateSync);
        app.init_schedule(ImeCandidateSync)
            .init_resource::<ImeState>()
            .init_resource::<ImeCandidateArea>()
            // NOTE: `ime_policy_system` must run unconditionally every frame.
            // Bevy's text-input plugin resets `Window::ime_enabled` every
            // `PreUpdate` whenever no `TextInput` has focus, so gating this
            // system with `run_if` would switch IME off in the terminal and in
            // webviews.
            .add_systems(
                Update,
                (ime_policy_system, read_ime_events)
                    .chain()
                    .in_set(InputPhase::Dispatch),
            )
            .add_systems(
                PostUpdate,
                reset_ime_after_rename
                    .after(RenameSystems::Finish)
                    .run_if(resource_exists_and_changed::<TabRename>),
            )
            .add_systems(
                ImeCandidateSync,
                // NOTE: on Windows, winit converts the area to physical pixels with the
                // scale factor current at the call and never converts it again. A move to a
                // monitor with another scale factor can leave the logical area unchanged, so
                // the area is sent again on every backend scale-factor change; without this,
                // the IME keeps the old physical position. A one-frame layout lag often
                // resends the area anyway, which hides a missing condition in manual testing.
                sync_ime_candidate_area.run_if(
                    resource_exists_and_changed::<ImeCandidateArea>
                        .or_eager(on_message::<WindowBackendScaleFactorChanged>),
                ),
            )
            .add_observer(apply_ime_commit_to_terminal);
    }
}

/// Validated snapshot of a preedit string and its UTF-8-safe caret
/// range.
#[derive(Debug)]
pub(crate) struct Composition {
    text: String,
    caret: Option<(usize, usize)>,
}

impl Composition {
    /// Validates and constructs a `Composition`. Returns `None` when:
    ///   - `text` is empty (an empty-value `Preedit` means "no composition").
    ///
    /// Sets `caret = None` when:
    ///   - either endpoint is out of bounds (`> text.len()`);
    ///   - either endpoint lands on a non-UTF-8 boundary byte;
    ///   - `begin > end`.
    ///
    /// `begin == end` is the normal caret-only case. `begin != end`
    /// represents a clause-selection range (macOS IME during clause
    /// conversion, etc.).
    pub(crate) fn try_new(text: String, raw_caret: Option<(usize, usize)>) -> Option<Self> {
        if text.is_empty() {
            return None;
        }
        let caret = match raw_caret {
            None => None,
            Some((begin, end)) => {
                let valid = begin <= end
                    && end <= text.len()
                    && text.is_char_boundary(begin)
                    && text.is_char_boundary(end);
                if valid { Some((begin, end)) } else { None }
            }
        };
        Some(Composition { text, caret })
    }

    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    pub(crate) fn caret(&self) -> Option<(usize, usize)> {
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
/// whether IME is allowed; this resource does not mirror it.
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
/// returns the text that should be committed to the active pane (only set on
/// `Ime::Commit`).
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

/// Resolves the single keyboard-focused terminal surface — the `KeyboardFocused`
/// `OrzmaTerminal` surface. Returns `None` when zero or more than one entity is
/// focused; both degrade to "no surface". The host maintains the "exactly one
/// focused" invariant.
pub(crate) fn resolve_focused_surface(
    focused: &Query<Entity, With<KeyboardFocused>>,
) -> Option<Entity> {
    focused.single().ok()
}

#[derive(Resource, Debug, Default, PartialEq)]
struct ImeCandidateArea(Option<Rect>);

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, PartialOrd, Ord, ScheduleLabel)]
struct ImeCandidateSync;

/// Derives the window's IME policy from keyboard focus and writes it to the
/// primary window and to `ImeCandidateArea`.
///
/// The IME is switched off only when nothing that takes IME input holds
/// keyboard focus or the focused terminal is in vi mode; a focused webview
/// keeps it on even over a vi-mode terminal. A focused terminal anchors
/// `ime_position` at its cursor cell's top-left corner and publishes that
/// cell, in logical pixels, as `ImeCandidateArea`; while the area holds
/// `Some`, `ime_position` equals its `min`. A focused inline webview anchors
/// `ime_position` at its overlay rect origin. Every other target leaves
/// `ime_position` unchanged and clears the area.
///
/// While a tab rename is active the IME stays on, and the rename field
/// positions the candidate window.
fn ime_policy_system(
    mut area: ResMut<ImeCandidateArea>,
    mut primary_window: Query<&mut Window, With<PrimaryWindow>>,
    params: ImePolicyParams,
) {
    let Ok(mut window) = primary_window.single_mut() else {
        return;
    };
    let target = params.decide(window.resolution.scale_factor());
    let enabled = !matches!(target, ImeTarget::Disabled);
    if window.ime_enabled != enabled {
        window.ime_enabled = enabled;
    }
    let (rect, anchor) = match target {
        ImeTarget::Terminal(Some(rect)) => (Some(rect), Some(rect.min)),
        ImeTarget::Webview(Some(pos)) => (None, Some(pos)),
        _ => (None, None),
    };
    if let Some(anchor) = anchor
        && window.ime_position != anchor
    {
        window.ime_position = anchor;
    }
    area.set_if_neq(ImeCandidateArea(rect));
}

/// Tells the primary window's IME which rectangle its candidate window must
/// not cover.
///
/// The rectangle is in logical pixels of the window's client area, with the
/// origin at the top-left corner and the y axis pointing down. It replaces
/// the 10 × 10 area bevy_winit derives from `Window::ime_position`. While
/// `ImeCandidateArea` holds `Some`, `ime_position` must change only together
/// with it: if `ime_position` changes alone, bevy_winit overwrites the
/// rectangle with its 10 × 10 area and the rectangle is not restored.
///
/// Nothing happens when `ImeCandidateArea` holds `None`, when there is no
/// primary window, or when no winit window backs the primary window.
///
/// Must run after bevy_winit hands `ime_position` to winit in the same
/// frame; running first lets the 10 × 10 area overwrite the rectangle.
///
/// On X11 the size is ignored, and only the origin reaches the input method
/// as its spot.
fn sync_ime_candidate_area(
    area: Res<ImeCandidateArea>,
    primary_window: Query<Entity, With<PrimaryWindow>>,
    _non_send_marker: NonSendMarker,
) {
    let Some(rect) = area.0 else {
        return;
    };
    let Ok(entity) = primary_window.single() else {
        return;
    };
    // NOTE: bevy_winit keeps `WinitWindows` in a main-thread thread-local, not
    // in the world. `NonSendMarker` pins this system to the main thread, so the
    // borrow sees the populated instance rather than an empty one.
    WINIT_WINDOWS.with_borrow(|winit_windows| {
        if let Some(window) = winit_windows.get_window(entity) {
            window.set_ime_cursor_area(
                LogicalPosition::new(rect.min.x, rect.min.y),
                LogicalSize::new(rect.width(), rect.height()),
            );
        }
    });
}

/// Drains `Ime` events, updates `ImeState`, and on `Ime::Commit` triggers
/// `ImeCommit` to the keyboard-focused surface. The commit is suppressed (the
/// state machine still runs, so `ImeState` stays consistent) when EITHER any
/// webview owns keyboard focus OR the focused surface is in vi mode. While a
/// tab rename is active the events are dropped unread; they belong to the
/// rename field.
fn read_ime_events(
    mut commands: Commands,
    mut events: MessageReader<Ime>,
    mut state: ResMut<ImeState>,
    rename: Option<Res<TabRename>>,
    focused: Query<Entity, With<KeyboardFocused>>,
    focused_webview: Res<FocusedWebview>,
    vi_modes: Query<(), With<ViModeState>>,
) {
    if rename.as_deref().is_some_and(TabRename::is_active) {
        events.clear();
        return;
    }
    let surface = resolve_focused_surface(&focused);
    for event in events.read() {
        let Some(commit_text) = apply_event(&mut state, event) else {
            continue;
        };
        // NOTE: gate on `FocusedWebview` itself, NOT on "is the focused webview a
        // child of `surface`". A focused webview consumes the winit Ime events via
        // bevy_cef, and focus can still sit on a webview of a pane that is no
        // longer the active surface until the host's release reaches the GUI — a
        // surface-relative check would miss it and inject the commit into the
        // newly-active pane's shell.
        if focused_webview.0.is_some() {
            continue;
        }
        if commit_text.is_empty() {
            continue;
        }
        let Some(entity) = surface else {
            tracing::warn!(
                target: "orzma::input::ime",
                "IME commit dropped: no keyboard-focused surface",
            );
            continue;
        };
        if vi_modes.contains(entity) {
            continue;
        }
        commands.trigger(ImeCommit {
            entity,
            text: commit_text,
        });
    }
}

/// Drops an open composition when a tab rename starts or ends, so a
/// composition cut off by the switch does not keep terminal keys
/// suppressed.
fn reset_ime_after_rename(mut state: ResMut<ImeState>) {
    if state.is_composing() {
        *state = ImeState::default();
    }
}

fn apply_ime_commit_to_terminal(
    ev: On<ImeCommit>,
    mut commands: Commands,
    terminals: Query<(), With<OrzmaTerminal>>,
) {
    if terminals.get(ev.entity).is_err() {
        return;
    }
    let Some(text) = KeyText::new(ev.text.clone()) else {
        return;
    };
    commands.trigger(RequestActiveKeyInput {
        key: TerminalKey::Character(text),
        modifiers: TerminalModifiers::default(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    mod composition {
        use super::*;

        #[test]
        fn try_new_returns_none_for_empty_text() {
            assert!(Composition::try_new(String::new(), None).is_none());
            assert!(Composition::try_new(String::new(), Some((0, 0))).is_none());
        }

        #[test]
        fn try_new_accepts_valid_caret() {
            let c = Composition::try_new("hello".into(), Some((3, 3))).unwrap();
            assert_eq!(c.text(), "hello");
            assert_eq!(c.caret(), Some((3, 3)));
        }

        #[test]
        fn try_new_accepts_caret_at_text_len() {
            let c = Composition::try_new("ab".into(), Some((2, 2))).unwrap();
            assert_eq!(c.caret(), Some((2, 2)));
        }

        #[test]
        fn try_new_clamps_out_of_bounds_caret_to_none() {
            let c = Composition::try_new("ab".into(), Some((99, 99))).unwrap();
            assert_eq!(c.text(), "ab");
            assert_eq!(c.caret(), None);
        }

        #[test]
        fn try_new_rejects_non_char_boundary_caret() {
            let c = Composition::try_new("あ".into(), Some((1, 1))).unwrap();
            assert_eq!(c.text(), "あ");
            assert_eq!(c.caret(), None);
        }

        #[test]
        fn try_new_preserves_clause_selection_range() {
            let c = Composition::try_new("hello".into(), Some((2, 5))).unwrap();
            assert_eq!(c.caret(), Some((2, 5)));
        }

        #[test]
        fn try_new_rejects_end_out_of_bounds() {
            let c = Composition::try_new("hello".into(), Some((1, 99))).unwrap();
            assert_eq!(c.caret(), None);
        }

        #[test]
        fn try_new_rejects_end_on_non_char_boundary() {
            let c = Composition::try_new("あい".into(), Some((0, 4))).unwrap();
            assert_eq!(c.caret(), None);
        }

        #[test]
        fn try_new_rejects_begin_greater_than_end() {
            let c = Composition::try_new("hello".into(), Some((3, 1))).unwrap();
            assert_eq!(c.caret(), None);
        }

        #[test]
        fn try_new_with_none_caret_keeps_none() {
            let c = Composition::try_new("hi".into(), None).unwrap();
            assert_eq!(c.caret(), None);
        }
    }

    mod apply_event {
        use super::*;

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
            assert_eq!(c.caret(), Some((3, 3)));
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
    }

    mod ime_policy_system {
        use super::*;
        use crate::input::ime::test_support::{
            focus_inline_webview, policy_app, spawn_focused_surface,
        };
        use bevy::ecs::system::RunSystemOnce;
        use bevy::math::{IVec4, Vec2};
        use bevy::prelude::default;
        use bevy::window::WindowResolution;
        use bevy_orzmux::prelude::TabId;

        /// Asserts that a focused webview switches the window's IME on when no
        /// terminal holds keyboard focus.
        ///
        /// Case: the user clicks into a webview while no pane holds keyboard focus
        /// and the window's IME is off.
        #[test]
        fn ime_stays_enabled_for_focused_webview() {
            let mut app = system_app();
            let webview = app.world_mut().spawn_empty().id();
            app.world_mut().resource_mut::<FocusedWebview>().0 = Some(webview);

            let (enabled, _) = run_policy(&mut app);

            assert!(
                enabled,
                "IME must stay enabled while a CEF webview owns focus, or bevy_cef's IME bridge is starved"
            );
        }

        /// The `ime_position` the primary window starts with, distinct from
        /// every anchor a case expects.
        const UNTOUCHED_POSITION: Vec2 = Vec2::new(-7.0, -7.0);

        /// An app holding every resource `ime_policy_system` reads, with 8×16 px
        /// cells and a primary window at a scale factor of 2 whose IME starts off
        /// at `UNTOUCHED_POSITION`.
        fn system_app() -> App {
            let mut app = policy_app();
            app.init_resource::<ImeCandidateArea>();
            app.world_mut().spawn((
                Window {
                    focused: true,
                    resolution: WindowResolution::new(1600, 1200).with_scale_factor_override(2.0),
                    ime_enabled: false,
                    ime_position: UNTOUCHED_POSITION,
                    ..default()
                },
                PrimaryWindow,
            ));
            app
        }

        fn switch_window_ime_on(app: &mut App) {
            let mut windows = app
                .world_mut()
                .query_filtered::<&mut Window, With<PrimaryWindow>>();
            windows
                .single_mut(app.world_mut())
                .expect("one primary window")
                .ime_enabled = true;
        }

        /// Runs the policy once and returns the primary window's IME switch and
        /// anchor.
        fn run_policy(app: &mut App) -> (bool, Vec2) {
            app.world_mut()
                .run_system_once(ime_policy_system)
                .expect("the policy runs");
            let mut windows = app
                .world_mut()
                .query_filtered::<&Window, With<PrimaryWindow>>();
            let window = windows.single(app.world()).expect("one primary window");
            (window.ime_enabled, window.ime_position)
        }

        /// Asserts that a keyboard-focused terminal switches the window's IME on
        /// and anchors the candidate window at its cursor cell in logical pixels.
        ///
        /// Case: orzma has just opened with the IME off, and the first frame with
        /// a focused shell pane runs on a display with a scale factor of 2.
        #[test]
        fn a_focused_surface_switches_the_window_ime_on_at_its_cursor_cell() {
            let mut app = system_app();
            spawn_focused_surface(&mut app);

            let (enabled, position) = run_policy(&mut app);

            assert!(
                enabled,
                "the first focused-surface tick switches the IME on"
            );
            assert!(
                position.abs_diff_eq(Vec2::new(70.0, 130.0), 1e-3),
                "the anchor is the cursor cell's top-left in logical px, got {position:?}",
            );
        }

        /// Asserts that a focused inline webview moves the window's anchor to its
        /// overlay rect origin in logical pixels.
        ///
        /// Case: the user clicks into an inline webview the active pane mounted
        /// at row 2, column 3, on a display with a scale factor of 2.
        #[test]
        fn a_focused_inline_webview_moves_the_window_anchor_to_its_overlay_rect() {
            let mut app = system_app();
            let surface = spawn_focused_surface(&mut app);
            focus_inline_webview(&mut app, surface, IVec4::new(2, 3, 4, 10));

            let (enabled, position) = run_policy(&mut app);

            assert!(enabled, "a focused webview keeps the IME on");
            assert!(
                position.abs_diff_eq(Vec2::new(62.0, 66.0), 1e-3),
                "the anchor is the overlay rect origin in logical px, got {position:?}",
            );
        }

        /// Asserts that a tab rename switches the window's IME on and leaves the
        /// anchor to the rename field.
        ///
        /// Case: the user double-clicks a tab to rename it while the shell pane
        /// below still holds keyboard focus.
        #[test]
        fn a_tab_rename_switches_the_window_ime_on_without_moving_its_anchor() {
            let mut app = system_app();
            app.insert_resource(TabRename::active_for_test(TabId(1)));
            spawn_focused_surface(&mut app);

            let (enabled, position) = run_policy(&mut app);

            assert!(enabled, "a tab rename keeps the IME on");
            assert_eq!(position, UNTOUCHED_POSITION);
        }

        /// Asserts that the window's IME switches off when neither a terminal nor
        /// a webview holds keyboard focus.
        ///
        /// Case: the last pane with focus goes away while the IME was on.
        #[test]
        fn no_focused_surface_and_no_webview_switches_the_window_ime_off() {
            let mut app = system_app();
            switch_window_ime_on(&mut app);

            let (enabled, _) = run_policy(&mut app);

            assert!(!enabled);
        }

        /// Asserts that a keyboard-focused terminal in vi mode switches the
        /// window's IME off.
        ///
        /// Case: the user enters vi mode in the focused pane while the IME was
        /// on.
        #[test]
        fn a_vi_mode_surface_switches_the_window_ime_off() {
            let mut app = system_app();
            switch_window_ime_on(&mut app);
            let surface = spawn_focused_surface(&mut app);
            app.world_mut().entity_mut(surface).insert(ViModeState);

            let (enabled, _) = run_policy(&mut app);

            assert!(!enabled);
        }

        /// Asserts that an inline webview on an inactive overlay slot keeps the
        /// window's IME on but leaves its anchor where it was.
        ///
        /// Case: the user focuses an inline webview in the frame its pane
        /// released the overlay slot, before the focus moves away.
        #[test]
        fn an_inline_webview_on_an_inactive_slot_leaves_the_window_anchor_unchanged() {
            let mut app = system_app();
            let surface = spawn_focused_surface(&mut app);
            focus_inline_webview(&mut app, surface, IVec4::new(2, 3, 0, 10));

            let (enabled, position) = run_policy(&mut app);

            assert!(enabled, "a focused webview keeps the IME on");
            assert_eq!(position, UNTOUCHED_POSITION);
        }

        /// Asserts that a focused inline webview keeps the window's IME on even
        /// when its terminal is in vi mode.
        ///
        /// Case: the user enters vi mode in a pane, then clicks into the inline
        /// webview that pane mounted.
        #[test]
        fn a_focused_inline_webview_keeps_the_window_ime_on_over_a_vi_mode_surface() {
            let mut app = system_app();
            let surface = spawn_focused_surface(&mut app);
            app.world_mut().entity_mut(surface).insert(ViModeState);
            focus_inline_webview(&mut app, surface, IVec4::new(2, 3, 4, 10));

            let (enabled, _) = run_policy(&mut app);

            assert!(enabled);
        }

        /// Asserts that a keyboard-focused terminal publishes its cursor cell, in
        /// logical pixels, as the candidate area.
        ///
        /// Case: the user starts composing Japanese text at the shell prompt on a
        /// display with a scale factor of 2.
        #[test]
        fn a_focused_surface_publishes_its_cursor_cell_as_the_candidate_area() {
            let mut app = system_app();
            spawn_focused_surface(&mut app);

            run_policy(&mut app);

            let rect = app
                .world()
                .resource::<ImeCandidateArea>()
                .0
                .expect("a focused terminal publishes a candidate area");
            assert!(
                rect.min.abs_diff_eq(Vec2::new(70.0, 130.0), 1e-3),
                "the area starts at the cursor cell's top-left, got {:?}",
                rect.min,
            );
            assert!(
                rect.max.abs_diff_eq(Vec2::new(74.0, 138.0), 1e-3),
                "the area ends at the cursor cell's bottom-right, got {:?}",
                rect.max,
            );
        }

        /// Asserts that a keyboard-focused terminal in vi mode clears the candidate
        /// area.
        ///
        /// Case: the user was composing at the prompt, then enters vi mode in the
        /// same pane.
        #[test]
        fn a_vi_mode_surface_clears_the_candidate_area() {
            let mut app = system_app();
            app.insert_resource(ImeCandidateArea(Some(Rect::new(70.0, 130.0, 74.0, 138.0))));
            let surface = spawn_focused_surface(&mut app);
            app.world_mut().entity_mut(surface).insert(ViModeState);

            run_policy(&mut app);

            assert_eq!(app.world().resource::<ImeCandidateArea>().0, None);
        }

        /// Asserts that an active tab rename clears the candidate area.
        ///
        /// Case: the user was composing at the prompt, then double-clicks a tab
        /// to rename it while the composition is still open.
        #[test]
        fn a_tab_rename_clears_the_candidate_area() {
            let mut app = system_app();
            app.insert_resource(ImeCandidateArea(Some(Rect::new(70.0, 130.0, 74.0, 138.0))));
            app.insert_resource(TabRename::active_for_test(TabId(1)));
            spawn_focused_surface(&mut app);

            run_policy(&mut app);

            assert_eq!(app.world().resource::<ImeCandidateArea>().0, None);
        }

        /// Asserts that an inline webview on an inactive overlay slot clears the
        /// candidate area.
        ///
        /// Case: the user was composing at the prompt, then focuses an inline
        /// webview in the frame its pane released the overlay slot, before the
        /// focus moves away.
        #[test]
        fn an_inline_webview_on_an_inactive_slot_clears_the_candidate_area() {
            let mut app = system_app();
            app.insert_resource(ImeCandidateArea(Some(Rect::new(70.0, 130.0, 74.0, 138.0))));
            let surface = spawn_focused_surface(&mut app);
            focus_inline_webview(&mut app, surface, IVec4::new(2, 3, 0, 10));

            run_policy(&mut app);

            assert_eq!(app.world().resource::<ImeCandidateArea>().0, None);
        }

        /// Asserts that no focused surface and no webview clears the candidate
        /// area.
        ///
        /// Case: the user was composing at the prompt, then closes the last
        /// pane before finishing the composition.
        #[test]
        fn no_focused_surface_and_no_webview_clears_the_candidate_area() {
            let mut app = system_app();
            app.insert_resource(ImeCandidateArea(Some(Rect::new(70.0, 130.0, 74.0, 138.0))));

            run_policy(&mut app);

            assert_eq!(app.world().resource::<ImeCandidateArea>().0, None);
        }

        /// Asserts that a focused surface without a laid-out node clears the
        /// candidate area.
        ///
        /// Case: the user was composing at the prompt, then focuses a newly
        /// opened pane before its first layout runs.
        #[test]
        fn a_focused_surface_without_a_laid_out_node_clears_the_candidate_area() {
            let mut app = system_app();
            app.insert_resource(ImeCandidateArea(Some(Rect::new(70.0, 130.0, 74.0, 138.0))));
            app.world_mut().spawn((OrzmaTerminal, KeyboardFocused));

            run_policy(&mut app);

            assert_eq!(app.world().resource::<ImeCandidateArea>().0, None);
        }
    }

    mod sync_ime_candidate_area {
        use super::*;
        use bevy::ecs::system::RunSystemOnce;
        use bevy::math::Vec2;
        use bevy::prelude::MinimalPlugins;

        /// The terminal's cursor cell at (70, 130) in logical pixels, one 4 × 8
        /// cell large.
        const CURSOR_CELL: Rect = Rect {
            min: Vec2::new(70.0, 130.0),
            max: Vec2::new(74.0, 138.0),
        };

        /// An app holding `area` as the candidate area and no primary window.
        fn sync_app(area: Option<Rect>) -> App {
            let mut app = App::new();
            app.add_plugins(MinimalPlugins);
            app.insert_resource(ImeCandidateArea(area));
            app
        }

        /// Spawns a primary window that no winit window backs.
        fn spawn_primary_window(app: &mut App) {
            app.world_mut().spawn((Window::default(), PrimaryWindow));
        }

        /// Asserts that the sync does nothing when the candidate area holds no
        /// rectangle.
        ///
        /// Case: the user starts renaming a tab, so the candidate area that held
        /// the terminal's cursor cell is cleared.
        #[test]
        fn an_empty_candidate_area_does_nothing() {
            let mut app = sync_app(None);
            spawn_primary_window(&mut app);

            let result = app.world_mut().run_system_once(sync_ime_candidate_area);

            assert!(result.is_ok());
        }

        /// Asserts that the sync does nothing when there is no primary window.
        ///
        /// Case: the primary window has been closed while the candidate area
        /// still holds the terminal's cursor cell.
        #[test]
        fn a_candidate_area_without_a_primary_window_does_nothing() {
            let mut app = sync_app(Some(CURSOR_CELL));

            let result = app.world_mut().run_system_once(sync_ime_candidate_area);

            assert!(result.is_ok());
        }

        /// Asserts that the sync does nothing when no winit window backs the
        /// primary window.
        ///
        /// Case: the primary window entity has been spawned, but winit has not
        /// yet created the native window behind it.
        #[test]
        fn a_primary_window_without_a_winit_window_does_nothing() {
            let mut app = sync_app(Some(CURSOR_CELL));
            spawn_primary_window(&mut app);

            let result = app.world_mut().run_system_once(sync_ime_candidate_area);

            assert!(result.is_ok());
        }
    }

    mod read_ime_events {
        use super::*;
        use bevy::ecs::hierarchy::ChildOf;
        use bevy::prelude::{MinimalPlugins, default};
        use bevy::state::app::StatesPlugin;
        use bevy::window::WindowResolution;
        use bevy_orzma_webview::Webview;
        use bevy_orzmux::prelude::TabId;
        use orzma_vt::prelude::InstanceId;
        use orzma_webview_host::prelude::MountId;

        fn build_app_with_focused_terminal() -> (App, Entity) {
            let mut app = App::new();
            app.add_plugins((MinimalPlugins, StatesPlugin))
                .add_systems(Update, read_ime_events);
            app.init_resource::<ImeState>();
            app.init_resource::<FocusedWebview>();
            app.add_message::<Ime>();

            let terminal_entity = app.world_mut().spawn((OrzmaTerminal, KeyboardFocused)).id();

            app.world_mut().spawn(Window {
                focused: true,
                resolution: WindowResolution::new(800, 600),
                ..default()
            });

            (app, terminal_entity)
        }

        #[derive(Resource, Default)]
        struct KeyInputs(usize);

        #[test]
        fn commit_consumes_state_with_focused_terminal() {
            // This asserts the state-machine side effects (commit clears the
            // composition) and that the focused-terminal commit path runs without
            // panic.
            let (mut app, _terminal_entity) = build_app_with_focused_terminal();

            app.world_mut()
                .resource_mut::<bevy::ecs::message::Messages<Ime>>()
                .write(Ime::Preedit {
                    window: Entity::PLACEHOLDER,
                    value: "こんに".into(),
                    cursor: Some((3, 3)),
                });
            app.update();
            assert!(
                app.world().resource::<ImeState>().is_composing(),
                "preedit must set the composition",
            );

            app.world_mut()
                .resource_mut::<bevy::ecs::message::Messages<Ime>>()
                .write(Ime::Commit {
                    window: Entity::PLACEHOLDER,
                    value: "こんにちは".into(),
                });
            app.update();

            assert!(
                !app.world().resource::<ImeState>().is_composing(),
                "commit must clear the composition",
            );
        }

        #[test]
        fn commit_suppressed_to_terminal_while_inline_focused() {
            let (mut app, terminal_entity) = build_app_with_focused_terminal();

            // Focus an inline child of the focused terminal.
            let child = app
                .world_mut()
                .spawn((
                    ChildOf(terminal_entity),
                    Webview::new("webview".into(), InstanceId(1), MountId::new(1), 0, 10, 40),
                ))
                .id();
            app.world_mut().resource_mut::<FocusedWebview>().0 = Some(child);

            app.world_mut()
                .resource_mut::<bevy::ecs::message::Messages<Ime>>()
                .write(Ime::Commit {
                    window: Entity::PLACEHOLDER,
                    value: "こんにちは".into(),
                });

            app.update();

            // The composition state machine still ran: ImeState cleared on commit,
            // even though the inline-focus arm suppresses the terminal write.
            assert!(
                !app.world().resource::<ImeState>().is_composing(),
                "the state machine must still consume the commit, leaving ImeState non-composing",
            );
        }

        #[test]
        fn commit_dropped_when_no_focused_terminal() {
            let (mut app, terminal_entity) = build_app_with_focused_terminal();
            // Remove the only focused terminal: the commit must be dropped (no target).
            app.world_mut().despawn(terminal_entity);

            app.world_mut()
                .resource_mut::<bevy::ecs::message::Messages<Ime>>()
                .write(Ime::Commit {
                    window: Entity::PLACEHOLDER,
                    value: "x".into(),
                });

            app.update();

            // The state machine still consumed the commit despite having no terminal.
            assert!(
                !app.world().resource::<ImeState>().is_composing(),
                "commit should clear the composition even when dropped",
            );
        }

        #[test]
        fn ime_commit_triggers_imecommit_for_focused_surface() {
            use bevy::prelude::On;

            #[derive(Resource, Default)]
            struct Hits(Vec<(Entity, String)>);

            let mut app = App::new();
            app.add_plugins((MinimalPlugins, StatesPlugin))
                .add_systems(Update, read_ime_events);
            app.init_resource::<ImeState>();
            app.init_resource::<FocusedWebview>();
            app.init_resource::<Hits>();
            app.add_message::<Ime>();
            app.add_observer(|ev: On<ImeCommit>, mut hits: ResMut<Hits>| {
                hits.0.push((ev.entity, ev.text.clone()));
            });

            // A second, unfocused terminal: the commit must route to the
            // KeyboardFocused one, not merely "the only one".
            app.world_mut().spawn(OrzmaTerminal);
            let focused = app.world_mut().spawn((OrzmaTerminal, KeyboardFocused)).id();

            app.world_mut()
                .resource_mut::<bevy::ecs::message::Messages<Ime>>()
                .write(Ime::Commit {
                    window: Entity::PLACEHOLDER,
                    value: "あ".into(),
                });
            app.update();

            let hits = &app.world().resource::<Hits>().0;
            assert_eq!(hits.len(), 1);
            assert_eq!(hits[0].0, focused);
            assert_eq!(hits[0].1, "あ");
        }

        /// Asserts that IME events are dropped unread while a rename is active.
        ///
        /// Case: the user commits kanji in the rename field while a terminal
        /// pane still holds keyboard focus underneath.
        #[test]
        fn ime_events_are_dropped_while_renaming() {
            let (mut app, _terminal) = build_app_with_focused_terminal();
            app.init_resource::<KeyInputs>()
                .add_observer(apply_ime_commit_to_terminal)
                .add_observer(
                    |_ev: On<RequestActiveKeyInput>, mut hits: ResMut<KeyInputs>| {
                        hits.0 += 1;
                    },
                )
                .insert_resource(TabRename::active_for_test(TabId(1)));

            app.world_mut().write_message(Ime::Preedit {
                window: Entity::PLACEHOLDER,
                value: "かんじ".into(),
                cursor: Some((9, 9)),
            });
            app.update();
            assert!(!app.world().resource::<ImeState>().is_composing());

            app.world_mut().write_message(Ime::Commit {
                window: Entity::PLACEHOLDER,
                value: "漢字".into(),
            });
            app.update();
            assert_eq!(app.world().resource::<KeyInputs>().0, 0);
            assert!(!app.world().resource::<ImeState>().is_composing());
        }
    }

    mod reset_ime_after_rename {
        use super::*;
        use bevy::prelude::MinimalPlugins;

        /// Asserts that a rename starting or ending drops an open composition,
        /// so terminal keys are not suppressed afterwards.
        ///
        /// Case: the user starts converting kana in the rename field and clicks
        /// a pane before finishing.
        #[test]
        fn ending_a_rename_mid_composition_resets_the_ime_state() {
            let mut app = App::new();
            app.add_plugins(MinimalPlugins)
                .init_resource::<ImeState>()
                .add_systems(
                    PostUpdate,
                    reset_ime_after_rename.run_if(resource_exists_and_changed::<TabRename>),
                );
            app.world_mut().resource_mut::<ImeState>().0 =
                Composition::try_new("かん".into(), None);
            app.world_mut().insert_resource(TabRename::default());
            app.update();
            assert!(!app.world().resource::<ImeState>().is_composing());
        }
    }

    mod apply_ime_commit_to_terminal {
        use super::*;
        use bevy::prelude::MinimalPlugins;

        /// Asserts an `ImeCommit` on a plain terminal fires `RequestActiveKeyInput`
        /// carrying the committed text as `TerminalKey::Character`.
        ///
        /// Case: the user finishes an IME composition (e.g. types `あ` via a
        /// Japanese input method) over a terminal with no active webview.
        #[test]
        fn ime_commit_fires_request_active_key_input_for_plain_terminal() {
            use crate::input::ime::ImeCommit;
            use crate::surface::OrzmaTerminal;
            use orzma_tty::prelude::TerminalKey;

            #[derive(Resource, Default)]
            struct Hits(Vec<TerminalKey>);

            let mut app = App::new();
            app.add_plugins(MinimalPlugins)
                .init_resource::<Hits>()
                .add_observer(apply_ime_commit_to_terminal)
                .add_observer(|ev: On<RequestActiveKeyInput>, mut h: ResMut<Hits>| {
                    h.0.push(ev.key.clone());
                });

            let term = app.world_mut().spawn(OrzmaTerminal).id();
            app.world_mut().trigger(ImeCommit {
                entity: term,
                text: "あ".into(),
            });
            app.update();

            assert_eq!(
                app.world().resource::<Hits>().0,
                vec![TerminalKey::Character(KeyText::new("あ").unwrap())]
            );
        }
    }
}
