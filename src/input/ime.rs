//! IME composition state for the terminal overlay.
//!
//! Provides `Composition` (a validated preedit snapshot), `ImeState`
//! (the active-composition resource), `read_ime_events` (the Bevy
//! system that drains `Ime` events and forwards `Ime::Commit` text to
//! the attached terminal), and `ime_policy_system` (toggles
//! `Window::ime_enabled` and `.ime_position`).

#[cfg(not(feature = "thin-client"))]
use crate::ui::AddressBarFocus;
#[cfg(not(feature = "thin-client"))]
use crate::ui::TerminalSurfaceMarker;
#[cfg(not(feature = "thin-client"))]
use crate::ui::copy_mode::CopyModeState;
use bevy::app::Update;
use bevy::app::{App, Plugin};
use bevy::ecs::message::MessageReader;
use bevy::ecs::query::With;
use bevy::ecs::resource::Resource;
#[cfg(not(feature = "thin-client"))]
use bevy::ecs::schedule::IntoScheduleConfigs;
#[cfg(not(feature = "thin-client"))]
use bevy::ecs::system::Commands;
#[cfg(not(feature = "thin-client"))]
use bevy::ecs::system::Res;
use bevy::ecs::system::{Query, ResMut};
#[cfg(not(feature = "thin-client"))]
use bevy::math::Vec2;
use bevy::prelude::Entity;
#[cfg(not(feature = "thin-client"))]
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::Ime;
#[cfg(not(feature = "thin-client"))]
use bevy::window::{PrimaryWindow, Window};
#[cfg(not(feature = "thin-client"))]
use bevy_cef::prelude::FocusedWebview;
#[cfg(not(feature = "thin-client"))]
use bevy_terminal::{TerminalKey, TerminalModifiers};
#[cfg(not(feature = "thin-client"))]
use bevy_terminal_renderer::TerminalCellMetricsResource;
#[cfg(not(feature = "thin-client"))]
use bevy_terminal_renderer::prelude::TerminalGrid;
#[cfg(not(feature = "thin-client"))]
use ozmux_multiplexer::MultiplexerCommands;
use ozmux_multiplexer::{AttachedWorkspace, WorkspaceMarker};

/// Bevy plugin that registers `ImeState` and the IME-event handling
/// systems. Ordering: `ime_policy_system` runs before
/// `read_ime_events`, both run before `dispatch_focused_key` (the
/// `.after(read_ime_events)` constraint on `dispatch_focused_key` is
/// added in `OzmuxShortcutPlugin`).
pub struct ImePlugin;

impl Plugin for ImePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ImeState>();
        #[cfg(not(feature = "thin-client"))]
        app.add_systems(Update, (ime_policy_system, read_ime_events).chain());
        #[cfg(feature = "thin-client")]
        app.add_systems(Update, read_ime_events);
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
    pub(crate) fn composition(&self) -> Option<&Composition> {
        self.0.as_ref()
    }

    pub(crate) fn is_composing(&self) -> bool {
        self.0.is_some()
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
/// `ime_enabled` is `true` iff a CEF webview owns focus (it drives its own
/// IME through bevy_cef's `Ime` → CEF bridge), OR the attached surface
/// carries `TerminalSurfaceMarker` and does NOT have `CopyModeState`.
///
/// `ime_position` is the logical-pixel anchor for the OS candidate
/// window — computed from the attached terminal's `UiGlobalTransform`
/// translation + `TerminalGrid.cursor` × cell pitch, then divided by
/// the window scale factor.
#[cfg(not(feature = "thin-client"))]
pub(crate) fn ime_policy_system(
    mux: MultiplexerCommands,
    attached_workspace: Query<Entity, (With<WorkspaceMarker>, With<AttachedWorkspace>)>,
    terminals: Query<(), With<TerminalSurfaceMarker>>,
    copy_modes: Query<(), With<CopyModeState>>,
    anchors: Query<(&ComputedNode, &UiGlobalTransform, &TerminalGrid)>,
    metrics: Res<TerminalCellMetricsResource>,
    focused_webview: Res<FocusedWebview>,
    address_bar_focus: Res<AddressBarFocus>,
    webview_anchors: Query<(&ComputedNode, &UiGlobalTransform)>,
    mut primary_window: Query<&mut Window, With<PrimaryWindow>>,
) {
    let Ok(mut window) = primary_window.single_mut() else {
        return;
    };

    // NOTE: a focused CEF webview drives its own IME through bevy_cef's
    // `Ime` → CEF bridge. ozmux MUST keep `ime_enabled` true here, or
    // bevy_winit calls winit `set_ime_allowed(false)` and the OS delivers
    // no `Ime` events at all — starving that bridge so webview IME silently
    // breaks. Removing this branch reintroduces that bug. The browser address
    // bar (a native Bevy text input) releases CEF focus, so it appears here
    // instead: keep IME on and route commits via `apply_ime_to_address_bar`.
    if let Some(target) = focused_webview.0.or(address_bar_focus.0) {
        if !window.ime_enabled {
            window.ime_enabled = true;
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

    let Some(entity) = super::resolve_focused_terminal(&mux, &attached_workspace) else {
        if window.ime_enabled {
            window.ime_enabled = false;
        }
        return;
    };

    let is_terminal = terminals.get(entity).is_ok();
    let in_copy_mode = copy_modes.get(entity).is_ok();
    let desired = is_terminal && !in_copy_mode;

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
    let scale = window.resolution.scale_factor();
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
/// text to the attached terminal.
///
/// Modifiers are forced to `TerminalModifiers::default()` on commit:
/// `ozmux_vt::input::encode_key` converts
/// `Text("a")` to control byte `0x01` when `ctrl` is held, which would
/// silently corrupt a single-ASCII-letter IME commit (e.g., the
/// macOS Character Viewer emoji path).
#[cfg(not(feature = "thin-client"))]
pub(crate) fn read_ime_events(
    mut events: MessageReader<Ime>,
    mut state: ResMut<ImeState>,
    mut commands: Commands,
    mux: MultiplexerCommands,
    attached_workspace: Query<Entity, (With<WorkspaceMarker>, With<AttachedWorkspace>)>,
) {
    for event in events.read() {
        if let Some(commit_text) = apply_event(&mut state, event) {
            let Some(workspace) = attached_workspace.iter().next() else {
                tracing::warn!(
                    target: "ozmux_gui::input::ime",
                    "commit dropped: no attached terminal",
                );
                continue;
            };
            super::forward_to_active_terminal(
                &mut commands,
                &mux,
                workspace,
                TerminalKey::Text(commit_text),
                TerminalModifiers::default(),
            );
        }
    }
}

/// Thin-client IME handler: drains `Ime` events, mutates `ImeState`, and sends
/// `Ime::Commit` text to the daemon as `ClientMessage::Input` over the wire.
#[cfg(feature = "thin-client")]
pub(crate) fn read_ime_events(
    mut conn: bevy::ecs::system::NonSendMut<crate::thin_client::ThinClientConn>,
    mut state: ResMut<ImeState>,
    mut events: MessageReader<Ime>,
    attached_workspace: Query<Entity, (With<WorkspaceMarker>, With<AttachedWorkspace>)>,
    active_panes: Query<&ozmux_multiplexer::ActivePane>,
    active_surfaces: Query<&ozmux_multiplexer::ActiveSurface>,
    surface_ids: Query<&ozmux_multiplexer::MuxSurfaceId>,
    surface_kinds: Query<&ozmux_multiplexer::SurfaceKind>,
) {
    for event in events.read() {
        if let Some(commit_text) = apply_event(&mut state, event).filter(|t| !t.is_empty()) {
            let Some(surface_ent) = crate::input::resolve_focused_terminal_readonly(
                &attached_workspace,
                &active_panes,
                &active_surfaces,
            ) else {
                continue;
            };
            if !crate::input::is_terminal_surface(&surface_kinds, surface_ent) {
                continue;
            }
            let Ok(surf_id) = surface_ids.get(surface_ent).map(|c| c.0) else {
                continue;
            };
            crate::thin_client::send_cmd(
                &mut conn,
                ozmux_proto::ClientMessage::Input {
                    surface: surf_id,
                    bytes: commit_text.into_bytes(),
                },
            );
        }
    }
}

#[cfg(all(test, not(feature = "thin-client")))]
mod tests {
    use super::*;
    use bevy::app::{App, Update};
    use bevy::ecs::entity::Entity;
    use bevy::ecs::observer::On;
    use bevy::ecs::resource::Resource;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::prelude::{MinimalPlugins, default};
    use bevy::window::{Ime, Window, WindowResolution};
    use bevy_terminal::{TerminalKey, TerminalKeyInput, TerminalModifiers};
    use ozmux_multiplexer::MultiplexerCommands;
    use ozmux_multiplexer::{AttachedWorkspace, MultiplexerPlugin, WorkspaceMarker};
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

    #[derive(Resource, Default, Clone)]
    struct CapturedKeys(Arc<Mutex<Vec<TerminalKeyInput>>>);

    fn capture_key_input(ev: On<TerminalKeyInput>, captured: Res<CapturedKeys>) {
        captured.0.lock().unwrap().push((*ev).clone());
    }

    fn build_app_with_attached_entity() -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(MultiplexerPlugin)
            .add_systems(Update, read_ime_events);
        app.init_resource::<ImeState>();
        app.insert_resource(CapturedKeys::default());
        app.add_observer(capture_key_input);
        app.add_message::<Ime>();

        let outcome = app
            .world_mut()
            .run_system_once(|mut mux: MultiplexerCommands| {
                mux.create_workspace(Some("default".into()))
            })
            .unwrap();
        app.world_mut().flush();
        app.world_mut()
            .entity_mut(outcome.workspace)
            .insert(AttachedWorkspace);

        // The Surface entity IS its own host: `resolve_focused_terminal` /
        // `forward_to_active_terminal` resolve directly to the active surface.
        let term_entity = outcome.surface;

        app.world_mut().spawn(Window {
            focused: true,
            resolution: WindowResolution::new(800, 600),
            ..default()
        });

        (app, term_entity)
    }

    #[test]
    fn ime_stays_enabled_for_focused_webview() {
        use bevy_terminal_renderer::CellMetrics;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(MultiplexerPlugin);
        app.init_resource::<FocusedWebview>();
        app.init_resource::<AddressBarFocus>();
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
    fn ime_stays_enabled_for_focused_address_bar() {
        use bevy_terminal_renderer::CellMetrics;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(MultiplexerPlugin);
        app.init_resource::<FocusedWebview>();
        app.init_resource::<AddressBarFocus>();
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

        // The address bar owns focus; FocusedWebview is released (None).
        let host = app.world_mut().spawn_empty().id();
        app.world_mut().resource_mut::<AddressBarFocus>().0 = Some(host);

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
            "IME must stay enabled while the browser address bar owns focus"
        );
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
            "modifiers MUST be default — see ozmux_vt::input::encode_key ctrl path",
        );
    }

    #[test]
    fn commit_dropped_when_no_attached_terminal() {
        let (mut app, _term_entity) = build_app_with_attached_entity();
        let attached: Vec<Entity> = app
            .world_mut()
            .query_filtered::<Entity, (With<WorkspaceMarker>, With<AttachedWorkspace>)>()
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
            "commit should be dropped when no AttachedWorkspace"
        );
    }
}
