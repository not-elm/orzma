//! IME composition state for the terminal overlay.
//!
//! Provides `Composition` (a validated preedit snapshot), `ImeState`
//! (the active-composition resource), `read_ime_events` (the Bevy
//! system that drains `Ime` events and forwards `Ime::Commit` text to
//! the active tmux pane), and `ime_policy_system` (toggles
//! `Window::ime_enabled` and `.ime_position`).

use crate::input::InputPhase;
use crate::ozma::AppMode;
use crate::ui::copy_mode::CopyModeState;
use crate::webview::inline::{Webview, focused_webview_of};
use bevy::app::{App, Plugin, Update};
use bevy::ecs::hierarchy::ChildOf;
use bevy::ecs::message::MessageReader;
use bevy::ecs::query::With;
use bevy::ecs::resource::Resource;
use bevy::ecs::schedule::IntoScheduleConfigs;
use bevy::ecs::system::{Commands, NonSend, Query, Res, ResMut, Single};
use bevy::math::Vec2;
use bevy::prelude::{Entity, State};
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::{Ime, PrimaryWindow, Window};
use bevy_cef::prelude::FocusedWebview;
use ozma_terminal::{KeyboardFocused, OzmaTerminal};
use ozma_tty_engine::{TerminalHandle, TerminalKey, TerminalKeyInput, TerminalModifiers};
use ozma_tty_renderer::TerminalCellMetricsResource;
use ozma_tty_renderer::prelude::{TerminalGrid, TerminalOverlays};
use ozmux_tmux::{ActivePane, TmuxConnection, TmuxPane, send_bytes_command};

/// Bevy plugin that registers `ImeState` and the IME-event handling
/// systems. Ordering: `ime_policy_system` runs before `read_ime_events`
/// (chained); both run in `InputPhase::Dispatch`, ahead of
/// `InputPhase::FocusedKey` where `forward_keys_to_tmux` forwards keys to the
/// active pane (and gates on `ImeState`, so IME must apply first).
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

/// Derives whether IME should be on this tick and writes
/// `PrimaryWindow.ime_enabled` and `.ime_position`.
///
/// `ime_enabled` is `true` iff a CEF webview owns focus (it drives its own
/// IME through bevy_cef's `Ime` → CEF bridge), OR a surface exists that does
/// NOT have `CopyModeState`. The surface is the active tmux pane, or — in
/// `AppMode::Ozma` with no active pane — the `KeyboardFocused` terminal; both
/// flow through the same copy-mode gate below.
///
/// `ime_position` is the logical-pixel anchor for the OS candidate
/// window — computed from the surface's `UiGlobalTransform`
/// translation + `TerminalGrid.cursor` × cell pitch, then divided by
/// the window scale factor. When the focused webview is an INLINE child of
/// the active pane, the anchor instead comes from that child's overlay
/// rect origin (`inline_ime_position`), since inline entities carry no UI
/// node for `webview_anchors` to read (spec §7).
pub(crate) fn ime_policy_system(
    mut primary_window: Query<&mut Window, With<PrimaryWindow>>,
    active_pane: Option<Single<(Entity, &TmuxPane), With<ActivePane>>>,
    copy_modes: Query<(), With<CopyModeState>>,
    anchors: Query<(&ComputedNode, &UiGlobalTransform, &TerminalGrid)>,
    metrics: Res<TerminalCellMetricsResource>,
    focused_webview: Res<FocusedWebview>,
    webview_anchors: Query<(&ComputedNode, &UiGlobalTransform)>,
    webview_parents: Query<&ChildOf, With<Webview>>,
    inline_slots: Query<&Webview>,
    overlays: Query<&TerminalOverlays>,
    current_mode: Res<State<AppMode>>,
    ozma_terminal: Query<Entity, (With<OzmaTerminal>, With<KeyboardFocused>)>,
) {
    let Ok(mut window) = primary_window.single_mut() else {
        return;
    };
    let active_surface = active_pane.map(|single| {
        let (entity, _) = *single;
        entity
    });

    // NOTE: a focused CEF webview drives its own IME through bevy_cef's
    // `Ime` → CEF bridge. ozmux MUST keep `ime_enabled` true here, or
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
        if let Some(child) =
            focused_webview_of(Some(&focused_webview), &webview_parents, active_surface)
            && let Some(pos) = inline_ime_position(
                window.resolution.scale_factor(),
                &webview_parents,
                &inline_slots,
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

    let surface = match active_surface {
        Some(entity) => Some(entity),
        // NOTE: Ozma mode has no tmux ActivePane; fall back to the focused terminal.
        None if *current_mode.get() == AppMode::Ozma => ozma_terminal.single().ok(),
        None => None,
    };
    let Some(entity) = surface else {
        if window.ime_enabled {
            window.ime_enabled = false;
        }
        return;
    };

    let in_copy_mode = copy_modes.get(entity).is_ok();
    let desired = !in_copy_mode;

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
    // `origin.y + size.height`. Bevy 0.18 hard-codes that size to
    // `PhysicalSize::new(10, 10)`
    // (`bevy_winit-0.18.1/src/system.rs:510`) with no way for us
    // to pass the actual cell height. Net effect: candidate window
    // appears one full row below the cursor — i.e. one row below
    // the preedit row, which is what users expect.
    //
    // NOTE: `UiGlobalTransform.translation` is the CENTER of the
    // node in PHYSICAL pixels (verified via Bevy 0.18 source:
    // `bevy_ui-0.18.1/src/layout/mod.rs:239-275` writes
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

/// Drains `Ime` events, mutates `ImeState`, and forwards `Ime::Commit`
/// text to the active tmux pane — UNLESS an inline webview owns focus, in
/// which case the commit-to-pane write is suppressed (bevy_cef commits it to
/// the page; see spec §7).
///
/// The commit text is sent verbatim via `send_bytes_command`
/// (`send-keys -H -t %<id> <hex…>`), which is byte-safe for UTF-8 multibyte
/// commits — including the macOS Character Viewer emoji path — without any
/// modifier interpretation.
pub(crate) fn read_ime_events(
    mut commands: Commands,
    mut events: MessageReader<Ime>,
    mut state: ResMut<ImeState>,
    mut handles: Query<&mut TerminalHandle>,
    connection: NonSend<TmuxConnection>,
    active_pane: Option<Single<(Entity, &TmuxPane), With<ActivePane>>>,
    focused_webview: Res<FocusedWebview>,
    webview_parents: Query<&ChildOf, With<Webview>>,
    current_mode: Res<State<AppMode>>,
    ozma_terminal: Query<Entity, (With<OzmaTerminal>, With<KeyboardFocused>)>,
    copy_modes: Query<(), With<CopyModeState>>,
) {
    let active = active_pane.map(|single| *single);
    let active_surface = active.map(|(e, _)| e);
    for event in events.read() {
        if let Some(commit_text) = apply_event(&mut state, event) {
            // NOTE: Inline-focus commit suppression (spec §7): bevy_cef's own IME
            // systems independently consume the winit `Ime` events for the
            // focused webview, so ozmux must NOT also commit this text to the
            // pane — doing so double-delivers the composition (once to the page,
            // once to the terminal). The state machine above still ran, so
            // `ImeState` stays consistent; only the pane write is skipped.
            if focused_webview_of(Some(&focused_webview), &webview_parents, active_surface).is_some()
            {
                continue;
            }
            if commit_text.is_empty() {
                continue;
            }
            match current_mode.get() {
                AppMode::Ozmux => {
                    let Some((entity, pane)) = active else {
                        tracing::warn!(
                            target: "ozmux_gui::input::ime",
                            "commit dropped: no active tmux pane",
                        );
                        continue;
                    };
                    if !copy_modes.contains(entity)
                        && let Ok(mut handle) = handles.get_mut(entity)
                        && handle.snap_to_bottom_vt_only()
                    {
                        handle.flush_emit(&mut commands, entity);
                    }
                    let Some(client) = connection.client() else {
                        continue;
                    };
                    let target = format!("%{}", pane.id.0);
                    if let Err(e) = client
                        .handle()
                        .send(&send_bytes_command(&target, commit_text.as_bytes()))
                    {
                        tracing::warn!(?e, "IME commit send failed");
                    }
                }
                AppMode::Ozma => {
                    // NOTE: bevy_cef delivers the commit to the webview independently; suppress here to prevent duplicate input.
                    if focused_webview.0.is_some() {
                        continue;
                    }
                    // NOTE: route the commit to the focused terminal but do NOT
                    // also filter on `KeyboardDisabled` — IME composition itself
                    // sets `KeyboardDisabled` (suppressing raw keys via
                    // `dispatch_input`), yet the commit must still land here.
                    let Ok(entity) = ozma_terminal.single() else {
                        continue;
                    };
                    commands.trigger(TerminalKeyInput {
                        entity,
                        key: TerminalKey::Text(commit_text),
                        modifiers: TerminalModifiers::default(),
                    });
                }
            }
        }
    }
}

/// The logical-pixel anchor for the OS candidate window when an inline webview
/// owns focus: the owning terminal node's top-left (physical px) plus the
/// child's active overlay rect origin (`rect.y × cell_w`, `rect.x × cell_h`),
/// divided by the window scale factor. `None` when the focus chain is gone
/// (child/terminal despawned, no terminal node, or a sentinel rect) — the
/// caller then leaves `ime_position` unchanged rather than mis-anchoring.
fn inline_ime_position(
    scale_factor: f32,
    webview_parents: &Query<&ChildOf, With<Webview>>,
    inline_slots: &Query<&Webview>,
    anchors: &Query<(&ComputedNode, &UiGlobalTransform, &TerminalGrid)>,
    overlays: &Query<&TerminalOverlays>,
    metrics: &TerminalCellMetricsResource,
    child: Entity,
) -> Option<Vec2> {
    let terminal = webview_parents.get(child).ok()?.parent();
    let slot = inline_slots.get(child).ok()?.slot;
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
    use bevy::app::App;
    use bevy::ecs::entity::Entity;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::prelude::{AppExtStates, MinimalPlugins, default};
    use bevy::state::app::StatesPlugin;
    use bevy::window::{Ime, Window, WindowResolution};
    use ozma_tty_renderer::CellMetrics;
    use ozma_tty_renderer::prelude::{Cursor, TerminalGrid};
    use ozmux_tmux::{ActivePane, PaneId, TmuxConnection, TmuxPane};
    use tmux_control_parser::CellDims;

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

    fn build_app_with_active_pane() -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin))
            .add_systems(Update, read_ime_events);
        app.init_resource::<ImeState>();
        app.init_resource::<FocusedWebview>();
        app.init_state::<AppMode>();
        // No live tmux client: `TmuxConnection::client()` returns None, so the
        // commit send is skipped. Tests assert the state-machine side effects
        // and the absence of a panic on the active-pane / suppression paths.
        app.insert_non_send_resource(TmuxConnection::default());
        app.add_message::<Ime>();

        let pane_entity = app
            .world_mut()
            .spawn((
                TmuxPane {
                    id: PaneId(1),
                    dims: CellDims {
                        width: 0,
                        height: 0,
                        xoff: 0,
                        yoff: 0,
                    },
                },
                ActivePane,
            ))
            .id();

        app.world_mut().spawn(Window {
            focused: true,
            resolution: WindowResolution::new(800, 600),
            ..default()
        });

        (app, pane_entity)
    }

    #[test]
    fn ime_stays_enabled_for_focused_webview() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin));
        app.init_resource::<FocusedWebview>();
        app.init_state::<AppMode>();
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
    fn ime_enabled_for_active_tmux_pane() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin));
        app.init_resource::<ImeState>();
        app.init_resource::<FocusedWebview>();
        app.init_state::<AppMode>();
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
            TmuxPane {
                id: PaneId(1),
                dims: CellDims {
                    width: 0,
                    height: 0,
                    xoff: 0,
                    yoff: 0,
                },
            },
            ActivePane,
            ComputedNode::default(),
            UiGlobalTransform::default(),
            TerminalGrid {
                cursor: Some(Cursor::default()),
                ..default()
            },
        ));

        // Window starts with IME OFF — the policy must turn it ON for the
        // active tmux pane.
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
            "IME must be enabled while a tmux pane is active and not in copy mode"
        );
    }

    #[test]
    fn commit_consumes_state_with_active_pane() {
        // The unit-test harness has no live tmux client, so the byte send is a
        // no-op; this asserts the state-machine side effects (commit clears the
        // composition) and that the active-pane commit path runs without panic.
        let (mut app, _pane_entity) = build_app_with_active_pane();

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
        use ozma_tty_renderer::prelude::TerminalOverlays;

        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin));
        app.init_resource::<FocusedWebview>();
        app.init_state::<AppMode>();
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
        let pane = app
            .world_mut()
            .spawn((
                TmuxPane {
                    id: PaneId(1),
                    dims: CellDims {
                        width: 0,
                        height: 0,
                        xoff: 0,
                        yoff: 0,
                    },
                },
                ActivePane,
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
                ChildOf(pane),
                Webview {
                    view_id: "inline".into(),
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
            "IME must stay enabled while an inline webview owns focus"
        );
        assert!(
            window.ime_position.abs_diff_eq(Vec2::new(24.0, 32.0), 1e-3),
            "candidate window must anchor at the inline rect origin (phys 24,32 / scale 1), got {:?}",
            window.ime_position,
        );
    }

    #[test]
    fn commit_suppressed_to_pane_while_inline_focused() {
        let (mut app, pane_entity) = build_app_with_active_pane();

        // Focus an inline child of the active pane.
        let child = app
            .world_mut()
            .spawn((
                ChildOf(pane_entity),
                Webview {
                    view_id: "inline".into(),
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
        // even though the inline-focus arm suppresses the pane write.
        assert!(
            !app.world().resource::<ImeState>().is_composing(),
            "the state machine must still consume the commit, leaving ImeState non-composing",
        );
    }

    #[test]
    fn commit_dropped_when_no_active_pane() {
        let (mut app, pane_entity) = build_app_with_active_pane();
        // Remove the only active pane: the commit must be dropped (no target).
        app.world_mut().despawn(pane_entity);

        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<Ime>>()
            .write(Ime::Commit {
                window: Entity::PLACEHOLDER,
                value: "x".into(),
            });

        app.update();

        // The state machine still consumed the commit despite having no pane.
        assert!(
            !app.world().resource::<ImeState>().is_composing(),
            "commit should clear the composition even when dropped",
        );
    }

    #[test]
    fn ime_commit_routes_to_focused_ozma_terminal() {
        use bevy::prelude::On;

        #[derive(Resource, Default)]
        struct Hits(Vec<Entity>);

        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin))
            .add_systems(Update, read_ime_events);
        app.init_resource::<ImeState>();
        app.init_resource::<FocusedWebview>();
        app.init_resource::<Hits>();
        app.init_state::<AppMode>();
        app.insert_non_send_resource(TmuxConnection::default());
        app.add_message::<Ime>();
        app.add_observer(|ev: On<TerminalKeyInput>, mut hits: ResMut<Hits>| {
            hits.0.push(ev.entity);
        });

        app.world_mut().spawn(OzmaTerminal);
        let focused = app.world_mut().spawn((OzmaTerminal, KeyboardFocused)).id();

        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<Ime>>()
            .write(Ime::Commit {
                window: Entity::PLACEHOLDER,
                value: "あ".into(),
            });
        app.update();

        assert_eq!(app.world().resource::<Hits>().0, vec![focused]);
    }

    #[test]
    fn ime_enabled_and_anchored_for_focused_ozma_terminal() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin));
        app.init_resource::<FocusedWebview>();
        app.init_state::<AppMode>();
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

        app.world_mut().spawn(OzmaTerminal);
        app.world_mut().spawn((
            OzmaTerminal,
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
            "IME must enable for the focused Ozma terminal even with another terminal present"
        );
        assert_eq!(
            window.ime_position,
            Vec2::new(0.0, 16.0),
            "candidate window anchors one row below the focused terminal's cursor"
        );
    }
}
