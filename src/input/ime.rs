//! IME composition state for the terminal overlay.
//!
//! Provides `Composition` (a validated preedit snapshot), `ImeState`
//! (the active-composition resource), `read_ime_events` (the Bevy
//! system that drains `Ime` events and triggers `ImeCommit` to the
//! keyboard-focused surface), and `ime_policy_system` (toggles
//! `Window::ime_enabled` and `.ime_position`).

use crate::input::InputPhase;
use crate::input::focus::KeyboardFocused;
use crate::ui::vi_mode::ViModeState;
use bevy::app::{App, Plugin, Update};
use bevy::ecs::entity::Entity;
use bevy::ecs::event::EntityEvent;
use bevy::ecs::hierarchy::ChildOf;
use bevy::ecs::message::MessageReader;
use bevy::ecs::query::With;
use bevy::ecs::resource::Resource;
use bevy::ecs::schedule::IntoScheduleConfigs;
use bevy::ecs::system::{Commands, Query, Res, ResMut};
use bevy::math::Vec2;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::{Ime, PrimaryWindow, Window};
use bevy_cef::prelude::FocusedWebview;
use orzma_tty_renderer::TerminalCellMetricsResource;
use orzma_tty_renderer::prelude::{TerminalGrid, TerminalOverlays};
use orzma_webview::{Webview, focused_webview_of};

/// IME-committed text destined for the keyboard-focused terminal surface.
///
/// The observer in `src/input/default_mode.rs` applies it, writing the local
/// PTY via `TerminalKeyInput`.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct ImeCommit {
    #[event_target]
    pub(crate) entity: Entity,
    pub(crate) text: String,
}

/// Bevy plugin that registers `ImeState` and the IME-event handling
/// systems. Ordering: `ime_policy_system` runs before `read_ime_events`
/// (chained); both run in `InputPhase::Dispatch`, ahead of
/// `InputPhase::FocusedKey`, whose dispatcher gates on `ImeState`, so IME
/// must apply first.
pub struct ImePlugin;

impl Plugin for ImePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ImeState>().add_systems(
            Update,
            (ime_policy_system, read_ime_events)
                .chain()
                .in_set(InputPhase::Dispatch),
        );
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
    ///   - `text` is empty (treat any empty-value Preedit as
    ///     "no composition").
    ///
    /// Sets `caret = None` when:
    ///   - either endpoint is out of bounds (`> text.len()`);
    ///   - either endpoint lands on a non-UTF-8 boundary byte
    ///     (defensive: winit returns byte offsets that we later slice into);
    ///   - `begin > end` (invariant violation; winit's spec is `(begin, end)`).
    ///
    /// `begin == end` is the normal caret-only case. `begin != end`
    /// represents a clause-selection range (macOS IME during clause
    /// conversion, etc.) and is rendered as a hollow block over the
    /// span by `position_ime_overlay`.
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
/// returns the text that should be committed to the active pane (only set on
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

/// Resolves the single keyboard-focused terminal surface — the `KeyboardFocused`
/// `OrzmaTerminal` surface. Returns `None` when zero or more than one entity is
/// focused; both degrade to "no surface". The host maintains the "exactly one
/// focused" invariant.
pub(crate) fn resolve_focused_surface(
    focused: &Query<Entity, With<KeyboardFocused>>,
) -> Option<Entity> {
    focused.single().ok()
}

/// Derives whether IME should be on this tick and writes
/// `PrimaryWindow.ime_enabled` and `.ime_position`.
///
/// `ime_enabled` is `true` iff a CEF webview owns focus (it drives its own
/// IME through bevy_cef's `Ime` → CEF bridge), OR a surface exists that does
/// NOT have `ViModeState`. The surface is the `KeyboardFocused` `OrzmaTerminal`
/// surface.
///
/// `ime_position` is the logical-pixel anchor for the OS candidate
/// window — computed from the surface's `UiGlobalTransform`
/// translation + `TerminalGrid.cursor` × cell pitch, then divided by
/// the window scale factor. When the focused webview is an INLINE child of
/// the active pane, the anchor instead comes from that child's overlay
/// rect origin (`webview_ime_position`), since inline entities carry no UI
/// node for `webview_anchors` to read (spec §7).
fn ime_policy_system(
    mut primary_window: Query<&mut Window, With<PrimaryWindow>>,
    focused: Query<Entity, With<KeyboardFocused>>,
    vi_modes: Query<(), With<ViModeState>>,
    anchors: Query<(&ComputedNode, &UiGlobalTransform, &TerminalGrid)>,
    metrics: Res<TerminalCellMetricsResource>,
    focused_webview: Res<FocusedWebview>,
    webview_anchors: Query<(&ComputedNode, &UiGlobalTransform)>,
    webview_parents: Query<&ChildOf, With<Webview>>,
    webview_slots: Query<&Webview>,
    overlays: Query<&TerminalOverlays>,
) {
    let Ok(mut window) = primary_window.single_mut() else {
        return;
    };
    let surface = resolve_focused_surface(&focused);

    // NOTE: a focused CEF webview drives its own IME through bevy_cef's
    // `Ime` → CEF bridge. orzma MUST keep `ime_enabled` true here, or
    // bevy_winit calls winit `set_ime_allowed(false)` and the OS delivers
    // no `Ime` events at all — starving that bridge so webview IME silently
    // breaks. Removing this branch reintroduces that bug.
    if let Some(target) = focused_webview.0 {
        if !window.ime_enabled {
            window.ime_enabled = true;
        }
        // NOTE: Inline arm (spec §7): an inline child has no UI node, so the
        // tab-webview `webview_anchors` arm below cannot anchor it. Derive the
        // candidate-window position from the owning terminal's node transform
        // plus the inline placement rect's origin — the SAME px conversion the
        // wheel/click hit-test uses (`webview_local_dip`'s `origin_phys`), so
        // composition appears at the inline rect, not the terminal cursor.
        if let Some(child) = focused_webview_of(Some(&focused_webview), &webview_parents, surface)
            && let Some(pos) = webview_ime_position(
                window.resolution.scale_factor(),
                &webview_parents,
                &webview_slots,
                &anchors,
                &overlays,
                &metrics,
                child,
            )
        {
            if window.ime_position != pos {
                window.ime_position = pos;
            }
            return;
        }
        if let Ok((node, ui_xform)) = webview_anchors.get(target) {
            let scale = window.resolution.scale_factor();
            let top_left_phys = ui_xform.translation - 0.5 * node.size();
            let pos = top_left_phys / scale;
            if window.ime_position != pos {
                window.ime_position = pos;
            }
        }
        return;
    }

    let Some(entity) = surface else {
        if window.ime_enabled {
            window.ime_enabled = false;
        }
        return;
    };

    let in_vi_mode = vi_modes.get(entity).is_ok();
    let desired = !in_vi_mode;

    if window.ime_enabled != desired {
        window.ime_enabled = desired;
    }

    if !desired {
        return;
    }

    // NOTE: Anchor `ime_position` at the TOP of the row BELOW the
    // cursor. This is intentionally a DIFFERENT anchor from the
    // inline overlay's `compute_overlay_pos`, which sits AT the
    // cursor row (Alacritty parity). The OS candidate window still
    // anchors one row down because macOS treats the rect
    // `set_ime_cursor_area(origin, size)` as the marked-text
    // bounding box and places the candidate window just below
    // `origin.y + size.height`. Bevy 0.19 hard-codes that size to
    // `PhysicalSize::new(10, 10)`
    // (`bevy_winit-0.19.0/src/system.rs:544-546`) with no way for us
    // to pass the actual cell height. Net effect: candidate window
    // appears one full row below the cursor — i.e. one row below
    // the preedit row, which is what users expect.
    //
    // NOTE: `UiGlobalTransform.translation` is the CENTER of the
    // node in PHYSICAL pixels (verified via Bevy 0.19 source:
    // `bevy_ui-0.19.0/src/layout/mod.rs:269-299` writes
    // `local_center` into the global transform; `ComputedNode.size`
    // is also physical px). To get the node's top-left in physical
    // px, subtract `0.5 * node.size()`. Do NOT multiply translation
    // by `scale` — it's already physical.
    let Ok((node, ui_xform, grid)) = anchors.get(entity) else {
        return;
    };
    let scale = window.resolution.scale_factor().max(f32::EPSILON);
    let cell_w_phys = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h_phys = metrics.metrics.line_height_phys.floor().max(1.0);
    let cursor_cell = grid.cursor.clone().unwrap_or_default();
    let host_origin_phys = ui_xform.translation - 0.5 * node.size();
    let cell_origin_phys = host_origin_phys
        + Vec2::new(
            cursor_cell.x as f32 * cell_w_phys,
            (cursor_cell.y as f32 + 1.0) * cell_h_phys,
        );
    let pos_logical = cell_origin_phys / scale;
    if window.ime_position != pos_logical {
        window.ime_position = pos_logical;
    }
}

/// Drains `Ime` events, updates `ImeState`, and on `Ime::Commit` triggers
/// `ImeCommit` to the keyboard-focused surface. The commit is suppressed (the
/// state machine still runs, so `ImeState` stays consistent) when EITHER any
/// webview owns keyboard focus OR the focused surface is in vi mode. The
/// commit transport is applied by the observer in `src/input/default_mode.rs`.
fn read_ime_events(
    mut commands: Commands,
    mut events: MessageReader<Ime>,
    mut state: ResMut<ImeState>,
    focused: Query<Entity, With<KeyboardFocused>>,
    focused_webview: Res<FocusedWebview>,
    vi_modes: Query<(), With<ViModeState>>,
) {
    let surface = resolve_focused_surface(&focused);
    for event in events.read() {
        let Some(commit_text) = apply_event(&mut state, event) else {
            continue;
        };
        // NOTE: gate on `FocusedWebview` itself, NOT on "is the focused webview a
        // child of `surface`". A focused webview consumes the winit Ime events via
        // bevy_cef, and `sync_focused_webview` deliberately keeps focus on an
        // inline webview even when its pane is no longer the active surface — a
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

/// The logical-pixel anchor for the OS candidate window when a webview
/// owns focus: the owning terminal node's top-left (physical px) plus the
/// child's active overlay rect origin (`rect.y × cell_w`, `rect.x × cell_h`),
/// divided by the window scale factor. `None` when the focus chain is gone
/// (child/terminal despawned, no terminal node, or a sentinel rect) — the
/// caller then leaves `ime_position` unchanged rather than mis-anchoring.
fn webview_ime_position(
    scale_factor: f32,
    webview_parents: &Query<&ChildOf, With<Webview>>,
    webview_slots: &Query<&Webview>,
    anchors: &Query<(&ComputedNode, &UiGlobalTransform, &TerminalGrid)>,
    overlays: &Query<&TerminalOverlays>,
    metrics: &TerminalCellMetricsResource,
    child: Entity,
) -> Option<Vec2> {
    let terminal = webview_parents.get(child).ok()?.parent();
    let slot = webview_slots.get(child).ok()?.slot;
    let (node, ui_xform, _) = anchors.get(terminal).ok()?;
    let rect = *overlays.get(terminal).ok()?.rects.get(usize::from(slot))?;
    if rect.z == 0 {
        return None;
    }
    let cell_w_phys = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h_phys = metrics.metrics.line_height_phys.floor().max(1.0);
    let host_origin_phys = ui_xform.translation - 0.5 * node.size();
    let rect_origin_phys = Vec2::new(rect.y as f32 * cell_w_phys, rect.x as f32 * cell_h_phys);
    Some((host_origin_phys + rect_origin_phys) / scale_factor.max(f32::EPSILON))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::OrzmaTerminal;
    use bevy::app::App;
    use bevy::ecs::entity::Entity;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::prelude::{MinimalPlugins, default};
    use bevy::state::app::StatesPlugin;
    use bevy::window::{Ime, Window, WindowResolution};
    use orzma_tty_renderer::CellMetrics;
    use orzma_tty_renderer::prelude::{Cursor, TerminalGrid};

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

    #[test]
    fn ime_stays_enabled_for_focused_webview() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin));
        app.init_resource::<FocusedWebview>();
        app.insert_resource(TerminalCellMetricsResource {
            metrics: CellMetrics {
                advance_phys: 8.0,
                line_height_phys: 16.0,
                ascent_phys: 12.0,
                descent_phys: 4.0,
                underline_position_phys: -2.0,
                underline_thickness_phys: 1.0,
                max_overflow_phys: 0.0,
            },
            phys_font_size: 12,
        });

        // A CEF webview owns focus; no terminal is active.
        let webview = app.world_mut().spawn_empty().id();
        app.world_mut().resource_mut::<FocusedWebview>().0 = Some(webview);

        // Window starts with IME OFF — the policy must turn it back ON.
        app.world_mut().spawn((
            Window {
                focused: true,
                resolution: WindowResolution::new(800, 600),
                ime_enabled: false,
                ..default()
            },
            PrimaryWindow,
        ));

        app.world_mut().run_system_once(ime_policy_system).unwrap();

        let mut q = app
            .world_mut()
            .query_filtered::<&Window, With<PrimaryWindow>>();
        let enabled = q.single(app.world()).expect("primary window").ime_enabled;
        assert!(
            enabled,
            "IME must stay enabled while a CEF webview owns focus, or bevy_cef's IME bridge is starved"
        );
    }

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
    fn ime_position_anchors_at_inline_rect_origin_for_focused_inline() {
        use orzma_tty_renderer::prelude::TerminalOverlays;

        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin));
        app.init_resource::<FocusedWebview>();
        app.insert_resource(TerminalCellMetricsResource {
            metrics: CellMetrics {
                advance_phys: 8.0,
                line_height_phys: 16.0,
                ascent_phys: 12.0,
                descent_phys: 4.0,
                underline_position_phys: -2.0,
                underline_thickness_phys: 1.0,
                max_overflow_phys: 0.0,
            },
            phys_font_size: 12,
        });

        // The active pane node spans the window with no transform → top-left at
        // (0, 0). Inline rect at rows 2.., cols 3.. → phys origin (24, 32) at
        // 8x16 px.
        let mut overlays = TerminalOverlays::default();
        overlays.rects[0] = bevy::math::IVec4::new(2, 3, 10, 40);
        let terminal = app
            .world_mut()
            .spawn((
                OrzmaTerminal,
                KeyboardFocused,
                ComputedNode {
                    size: Vec2::new(800.0, 600.0),
                    ..ComputedNode::DEFAULT
                },
                UiGlobalTransform::from_xy(400.0, 300.0),
                TerminalGrid::default(),
                overlays,
            ))
            .id();
        let child = app
            .world_mut()
            .spawn((
                ChildOf(terminal),
                Webview {
                    view_id: "webview".into(),
                    instance_id: None,
                    slot: 0,
                },
            ))
            .id();
        app.world_mut().resource_mut::<FocusedWebview>().0 = Some(child);

        app.world_mut().spawn((
            Window {
                focused: true,
                resolution: WindowResolution::new(800, 600),
                ime_enabled: false,
                ..default()
            },
            PrimaryWindow,
        ));

        app.world_mut().run_system_once(ime_policy_system).unwrap();

        let mut q = app
            .world_mut()
            .query_filtered::<&Window, With<PrimaryWindow>>();
        let window = q.single(app.world()).expect("primary window");
        assert!(
            window.ime_enabled,
            "IME must stay enabled while a webview owns focus"
        );
        assert!(
            window.ime_position.abs_diff_eq(Vec2::new(24.0, 32.0), 1e-3),
            "candidate window must anchor at the inline rect origin (phys 24,32 / scale 1), got {:?}",
            window.ime_position,
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
                Webview {
                    view_id: "webview".into(),
                    instance_id: None,
                    slot: 0,
                },
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

    #[test]
    fn ime_enabled_and_anchored_for_focused_orzma_terminal() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin));
        app.init_resource::<FocusedWebview>();
        app.insert_resource(TerminalCellMetricsResource {
            metrics: CellMetrics {
                advance_phys: 8.0,
                line_height_phys: 16.0,
                ascent_phys: 12.0,
                descent_phys: 4.0,
                underline_position_phys: -2.0,
                underline_thickness_phys: 1.0,
                max_overflow_phys: 0.0,
            },
            phys_font_size: 12,
        });

        app.world_mut().spawn(OrzmaTerminal);
        app.world_mut().spawn((
            OrzmaTerminal,
            KeyboardFocused,
            ComputedNode::default(),
            UiGlobalTransform::default(),
            TerminalGrid {
                cursor: Some(Cursor::default()),
                ..default()
            },
        ));
        app.world_mut().spawn((
            Window {
                focused: true,
                resolution: WindowResolution::new(800, 600),
                ime_enabled: false,
                ..default()
            },
            PrimaryWindow,
        ));

        app.world_mut().run_system_once(ime_policy_system).unwrap();

        let mut q = app
            .world_mut()
            .query_filtered::<&Window, With<PrimaryWindow>>();
        let window = q.single(app.world()).expect("primary window");
        assert!(
            window.ime_enabled,
            "IME must enable for the focused Orzma terminal even with another terminal present"
        );
        assert_eq!(
            window.ime_position,
            Vec2::new(0.0, 16.0),
            "candidate window anchors one row below the focused terminal's cursor"
        );
    }

    #[test]
    fn ime_enabled_for_keyboard_focused_terminal() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin));
        app.init_resource::<FocusedWebview>();
        app.insert_resource(TerminalCellMetricsResource {
            metrics: CellMetrics {
                advance_phys: 8.0,
                line_height_phys: 16.0,
                ascent_phys: 12.0,
                descent_phys: 4.0,
                underline_position_phys: -2.0,
                underline_thickness_phys: 1.0,
                max_overflow_phys: 0.0,
            },
            phys_font_size: 12,
        });
        app.world_mut().spawn((
            OrzmaTerminal,
            KeyboardFocused,
            ComputedNode::default(),
            UiGlobalTransform::default(),
            TerminalGrid {
                cursor: Some(Cursor::default()),
                ..default()
            },
        ));
        app.world_mut().spawn((
            Window {
                focused: true,
                resolution: WindowResolution::new(800, 600),
                ime_enabled: false,
                ..default()
            },
            PrimaryWindow,
        ));

        app.world_mut().run_system_once(ime_policy_system).unwrap();

        let mut q = app
            .world_mut()
            .query_filtered::<&Window, With<PrimaryWindow>>();
        assert!(
            q.single(app.world()).expect("primary window").ime_enabled,
            "IME must enable for a KeyboardFocused terminal",
        );
    }
}
