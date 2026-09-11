//! Focus & input suppression: defines `KeyboardFocused`, `KeyboardDisabled`,
//! and `MouseDisabled` — the three components the host uses to route keyboard
//! and mouse input — maintains the two `*Disabled` markers from the coarse
//! guards (IME, window focus, webview rect-claim, vi mode) in
//! `maintain_input_gates`, and keeps bevy_cef's `FocusedWebview` in step with
//! the active pane.

use crate::action::vi::mode::ViModeState;
use crate::configs::OrzmaConfigsResource;
use crate::input::InputPhase;
use crate::input::ime::ImeState;
use crate::surface::OrzmaTerminal;
use crate::surface::geometry::{cell_pitch_phys, phys_to_pane_local, topmost_surface_at};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::ui::{ComputedNode, ComputedStackIndex, UiGlobalTransform};
use bevy::window::{PrimaryWindow, Window};
use bevy_cef::prelude::{FocusedWebview, WebviewSource};
use bevy_orzma_tty_renderer::TerminalCellMetricsResource;
use bevy_orzma_tty_renderer::prelude::{PaneInactiveStyle, TerminalOverlays};
use bevy_orzma_webview::{NonInteractive, Webview, webview_hit_at};
use bevy_orzmux::prelude::{OrzmuxActivePaneChanged, OrzmuxPane, PaneAction, RequestPaneAction};
use orzma_configs::inactive_pane::InactivePaneConfig;

/// When present on an `OrzmaTerminal` entity, the crate's default keyboard
/// dispatcher skips it entirely — the host withholds keyboard input for it
/// (vi mode, a focused webview, IME composition, or an unfocused window).
#[derive(Component)]
pub(crate) struct KeyboardDisabled;

/// When present on an `OrzmaTerminal` entity, that terminal is the keyboard
/// focus: the crate's keyboard dispatcher routes raw keys to it, and the host
/// routes IME commits and anchors the OS candidate window to it. The host owns
/// focus policy and maintains the "exactly one focused" invariant; a terminal
/// with no `KeyboardFocused` receives no keyboard input.
#[derive(Component)]
pub(crate) struct KeyboardFocused;

/// When present on an `OrzmaTerminal` entity, the host's mouse dispatchers and
/// hover-cursor system skip it — it is removed from the hit-test candidate set,
/// so the pointer falls through to the next terminal below it. The host marks
/// every terminal `MouseDisabled` for modal suppression (picker / IME / focused
/// webview / unfocused window).
#[derive(Component)]
pub(crate) struct MouseDisabled;

/// A press landed on a pane surface.
#[derive(EntityEvent, Debug, Clone, Copy)]
pub(crate) struct PaneClicked {
    #[event_target]
    pub entity: Entity,
}

/// Registers `maintain_input_gates`, the webview focus-sync system, and the
/// active-pane / click-to-focus observers.
pub(super) struct FocusSyncPlugin;

impl Plugin for FocusSyncPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                maintain_input_gates.before(InputPhase::Hover),
                sync_focused_webview.after(InputPhase::FocusedKey),
            ),
        )
        .add_observer(on_active_pane_changed)
        .add_observer(on_pane_clicked);
    }
}

/// Applies an applied active-pane change, whether accepted from a
/// `Layout` or taken optimistically from a click: moves
/// `KeyboardFocused`, releases a focused webview that is not an inline
/// child of the new active pane, and swaps `PaneInactiveStyle` between
/// the previous and current panes. `previous` may already be despawned
/// (a `PaneClosed` in the same drain), hence the fallible commands.
fn on_active_pane_changed(
    ev: On<OrzmuxActivePaneChanged>,
    mut commands: Commands,
    mut focused_webview: ResMut<FocusedWebview>,
    configs: Res<OrzmaConfigsResource>,
    focused: Query<Entity, With<KeyboardFocused>>,
    webview_parents: Query<&ChildOf>,
) {
    for entity in focused.iter() {
        if Some(entity) != ev.current {
            commands.entity(entity).remove::<KeyboardFocused>();
        }
    }
    if let Some(previous) = ev.previous
        && let Ok(mut previous) = commands.get_entity(previous)
    {
        previous.try_insert(inactive_style(&configs.inactive_pane));
    }
    if let Some(current) = ev.current
        && let Ok(mut current) = commands.get_entity(current)
    {
        current.try_insert(KeyboardFocused);
        current.remove::<PaneInactiveStyle>();
    }
    let stays_focused = focused_webview.0.is_some_and(|child| {
        ev.current.is_some_and(|current| {
            webview_parents
                .get(child)
                .is_ok_and(|p| p.parent() == current)
        })
    });
    if !stays_focused && focused_webview.0.is_some() {
        focused_webview.0 = None;
    }
}

/// Click-to-focus: asks the bridge to select the clicked pane. The
/// bridge applies it as active at once and reports the change through
/// `OrzmuxActivePaneChanged`, so this frame's keys already go to the
/// clicked pane; the confirming `Layout` reconciles.
fn on_pane_clicked(
    ev: On<PaneClicked>,
    mut commands: Commands,
    panes: Query<(), With<OrzmuxPane>>,
) {
    if panes.get(ev.entity).is_err() {
        return;
    }
    commands.trigger(RequestPaneAction {
        action: PaneAction::Select(ev.entity),
    });
}

/// The renderer style for an inactive pane, from `[inactive_pane]`.
fn inactive_style(config: &InactivePaneConfig) -> PaneInactiveStyle {
    if !config.enabled {
        return PaneInactiveStyle::default();
    }
    let (r, g, b) = config.tint_color_rgb();
    let rgb = Color::srgb_u8(r, g, b).to_linear();
    PaneInactiveStyle {
        dim: config.dim,
        tint: Vec4::new(rgb.red, rgb.green, rgb.blue, config.tint),
        overlay_dim: config.webview_dim,
        overlay_desaturate: config.webview_desaturate,
    }
}

/// Keeps `bevy_cef`'s `FocusedWebview` in step with orzma's active pane.
///
/// bevy_cef only updates `FocusedWebview` when a *webview* node is clicked
/// (`set_focus_on_press`), so moving focus to a terminal pane (a non-webview)
/// leaves the webview focused: its DOM text area keeps the caret and
/// `send_key_event` keeps routing keystrokes to it. Driving `FocusedWebview`
/// from the active pane fixes both — keyboard follows the focused pane, and CEF
/// blurs the webview on focus-leave (`bevy_cef`'s `apply_webview_focus` releases
/// CEF focus when `FocusedWebview` becomes `None`).
///
/// One case is PRESERVED instead of driven: when `FocusedWebview` holds a
/// webview child (`Webview`) whose `ChildOf` parent is a live
/// `OrzmaTerminal` surface, that inline focus stands (spec §7, single
/// focus source). This covers click-granted focus and the app-declared
/// focus set via the control-plane `SetFocus` op. Releasing that focus
/// when the active pane moves elsewhere is `on_active_pane_changed`'s
/// job; this sync only clears it once the child despawns or focus moves
/// off it, falling through to the clear path below.
fn sync_focused_webview(
    mut focused: ResMut<FocusedWebview>,
    active_pane: Query<Entity, (With<OrzmaTerminal>, With<KeyboardFocused>)>,
    webviews: Query<(), With<WebviewSource>>,
    non_interactive: Query<(), With<NonInteractive>>,
    webview_parents: Query<&ChildOf, With<Webview>>,
    surfaces: Query<(), With<OrzmaTerminal>>,
) {
    // NOTE: a despawned inline child fails `webview_parents.get` here and so
    // falls through to the clear path below, which resolves to `None` and
    // clears it — that fall-through is the GC for surface inline focus; a
    // later edit that short-circuits this arm before the despawn check would
    // leak focus.
    if let Some(child) = focused.0
        && let Ok(parent) = webview_parents.get(child)
        && surfaces.contains(parent.parent())
    {
        return;
    }

    let active_surface = active_pane.iter().next();
    let active = active_surface
        .filter(|surface| webviews.contains(*surface) && !non_interactive.contains(*surface));
    if focused.0 != active {
        focused.0 = active;
    }
}

/// Returns `true` when host-side keyboard input should be suppressed: IME
/// composing, window not focused, or a webview owns the keyboard.
fn should_disable_input(composing: bool, window_focused: bool, webview_focused: bool) -> bool {
    composing || !window_focused || webview_focused
}

/// Inline-webview hit-test inputs for the mouse rect-claim, bundled to stay
/// within Bevy's system-parameter limit. `metrics` is optional so the gate still
/// runs before cell metrics exist (no claim possible yet).
#[derive(SystemParam)]
struct WebviewClaimParams<'w, 's> {
    metrics: Option<Res<'w, TerminalCellMetricsResource>>,
    surfaces: Query<
        'w,
        's,
        (
            Entity,
            &'static ComputedNode,
            &'static ComputedStackIndex,
            &'static UiGlobalTransform,
        ),
        With<OrzmaTerminal>,
    >,
    children: Query<'w, 's, &'static Children>,
    webviews: Query<'w, 's, (&'static Webview, Has<NonInteractive>)>,
    overlay_rects: Query<'w, 's, &'static TerminalOverlays>,
}

fn maintain_input_gates(
    mut commands: Commands,
    ime: Res<ImeState>,
    focused_webview: Res<FocusedWebview>,
    windows: Query<&Window, With<PrimaryWindow>>,
    terminals: Query<
        (
            Entity,
            Has<KeyboardDisabled>,
            Has<MouseDisabled>,
            Has<ViModeState>,
        ),
        With<OrzmaTerminal>,
    >,
    claim: WebviewClaimParams,
) {
    let window = windows.single().ok();
    let focused = window.map(|w| w.focused).unwrap_or(false);
    let keyboard_disable =
        should_disable_input(ime.is_composing(), focused, focused_webview.0.is_some());
    // NOTE: mouse is NOT disabled on webview focus alone — only over an
    // interactive inline rect (the rect-claim) — so an off-rect click still
    // reaches `dispatch_mouse_buttons` (and clears webview focus in the router).
    // Re-adding `focused_webview.0.is_some()` here would swallow that fallthrough
    // click, stranding the user on a focused webview.
    let mouse_modal = ime.is_composing() || !focused;
    let claimed = window.and_then(|w| cursor_claims_webview(w, &claim));
    for (entity, has_keyboard, has_mouse, in_vi_mode) in terminals.iter() {
        let disable_keyboard = keyboard_disable || in_vi_mode;
        let disable_mouse = mouse_modal || in_vi_mode || Some(entity) == claimed;
        if disable_keyboard && !has_keyboard {
            commands.entity(entity).insert(KeyboardDisabled);
        } else if !disable_keyboard && has_keyboard {
            commands.entity(entity).remove::<KeyboardDisabled>();
        }
        if disable_mouse && !has_mouse {
            commands.entity(entity).insert(MouseDisabled);
        } else if !disable_mouse && has_mouse {
            commands.entity(entity).remove::<MouseDisabled>();
        }
    }
}

/// The shell surface whose INTERACTIVE inline webview rect is under the cursor,
/// or `None`. Resolves the topmost `OrzmaTerminal` under the cursor, then
/// hit-tests its active overlay rects (`webview_hit_at` skips `NonInteractive`
/// children). A claimed surface is marked `MouseDisabled` so
/// `dispatch_mouse_buttons` yields the click to the webview router.
fn cursor_claims_webview(window: &Window, claim: &WebviewClaimParams) -> Option<Entity> {
    let metrics = claim.metrics.as_deref()?;
    let scale = window.scale_factor();
    let (cell_w, cell_h) = cell_pitch_phys(&metrics.metrics);
    let cursor_phys = window.cursor_position()? * scale;
    let terminal = topmost_surface_at(cursor_phys, claim.surfaces.iter())?;
    let (_, node, _, transform) = claim.surfaces.get(terminal).ok()?;
    let local_phys = phys_to_pane_local(node, transform, cursor_phys)?;
    let overlays = claim.overlay_rects.get(terminal).ok()?;
    webview_hit_at(
        &claim.children,
        &claim.webviews,
        overlays,
        terminal,
        local_phys,
        cell_w,
        cell_h,
        scale,
    )?;
    Some(terminal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orzma_vt::prelude::InstanceId;
    use orzmux::prelude::PaneId;

    #[test]
    fn focused_webview_follows_active_pane() {
        // Regression: moving focus to a terminal pane must clear FocusedWebview,
        // so bevy_cef blurs the webview (releasing its DOM text area
        // and stopping keyboard from routing to it). When the webview pane is
        // active, its webview must be focused.

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<FocusedWebview>();
        app.add_systems(Update, sync_focused_webview);

        // The active OrzmaTerminal IS the active surface. The webview pane carries a
        // WebviewSource; the terminal pane does not.
        let terminal_pane = app.world_mut().spawn(OrzmaTerminal).id();
        let ext_pane = app
            .world_mut()
            .spawn((OrzmaTerminal, WebviewSource::new("orzma://memo/index.html")))
            .id();

        let set_active = move |app: &mut App, active: Entity, inactive: Entity| {
            app.world_mut().entity_mut(active).insert(KeyboardFocused);
            app.world_mut()
                .entity_mut(inactive)
                .remove::<KeyboardFocused>();
            app.update();
        };

        set_active(&mut app, ext_pane, terminal_pane);
        assert_eq!(
            app.world().resource::<FocusedWebview>().0,
            Some(ext_pane),
            "active webview pane must focus its webview"
        );

        set_active(&mut app, terminal_pane, ext_pane);
        assert_eq!(
            app.world().resource::<FocusedWebview>().0,
            None,
            "moving focus to the terminal pane must clear the focused webview",
        );
    }

    #[test]
    fn non_interactive_webview_surface_never_takes_keyboard_focus() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<FocusedWebview>();
        app.add_systems(Update, sync_focused_webview);

        // The active OrzmaTerminal carries a NonInteractive WebviewSource: it must
        // never be focused.
        app.world_mut().spawn((
            OrzmaTerminal,
            KeyboardFocused,
            WebviewSource::new("orzma://memo/index.html"),
            NonInteractive,
        ));

        app.update();

        assert_eq!(
            app.world().resource::<FocusedWebview>().0,
            None,
            "NonInteractive webview surface must never become FocusedWebview"
        );
    }

    #[test]
    fn terminal_inline_focus_is_preserved() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<FocusedWebview>();
        app.add_systems(Update, sync_focused_webview);

        let pane = app.world_mut().spawn(OrzmaTerminal).id();
        let child = app
            .world_mut()
            .spawn((
                ChildOf(pane),
                Webview {
                    handle: "v".into(),
                    instance: InstanceId(1),
                    slot: 0,
                    rows: 10,
                    cols: 40,
                },
            ))
            .id();
        app.world_mut().resource_mut::<FocusedWebview>().0 = Some(child);

        app.update();

        assert_eq!(
            app.world().resource::<FocusedWebview>().0,
            Some(child),
            "an inline child of a live OrzmaTerminal must keep FocusedWebview across the per-frame sync",
        );
    }

    #[test]
    fn terminal_inline_focus_is_gc_on_despawn() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<FocusedWebview>();
        app.add_systems(Update, sync_focused_webview);

        let pane = app.world_mut().spawn(OrzmaTerminal).id();
        let child = app
            .world_mut()
            .spawn((
                ChildOf(pane),
                Webview {
                    handle: "v".into(),
                    instance: InstanceId(1),
                    slot: 0,
                    rows: 10,
                    cols: 40,
                },
            ))
            .id();
        app.world_mut().resource_mut::<FocusedWebview>().0 = Some(child);
        app.world_mut().entity_mut(child).despawn();

        app.update();

        assert_eq!(
            app.world().resource::<FocusedWebview>().0,
            None,
            "a despawned inline child must be GC'd out of FocusedWebview",
        );
    }

    #[test]
    fn sync_preserves_app_declared_inline_focus() {
        // NOTE: bevy_orzma_webview's apply_control_events and its supporting resource types
        // (OrzmaRegistry, ControlEvents, etc.) are pub(crate) and unreachable from the
        // binary. Setting FocusedWebview directly produces the same world state that
        // apply_control_events(SetFocus) would — the sync behavior under test is
        // identical regardless of how FocusedWebview was last written.

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<FocusedWebview>()
            .add_systems(Update, sync_focused_webview);

        let surface = app.world_mut().spawn(OrzmaTerminal).id();
        let child = app
            .world_mut()
            .spawn((
                ChildOf(surface),
                Webview {
                    handle: "h1".into(),
                    instance: InstanceId(1),
                    slot: 0,
                    rows: 10,
                    cols: 40,
                },
            ))
            .id();

        app.world_mut().resource_mut::<FocusedWebview>().0 = Some(child);

        app.update();
        assert_eq!(
            app.world().resource::<FocusedWebview>().0,
            Some(child),
            "app-declared inline focus must survive the per-frame sync_focused_webview"
        );

        app.world_mut().entity_mut(child).despawn();
        app.update();
        assert_eq!(
            app.world().resource::<FocusedWebview>().0,
            None,
            "app-declared focus must clear once its inline child despawns"
        );
    }

    /// Asserts that an accepted active change moves `KeyboardFocused`,
    /// tints the previous pane, and tolerates a despawned previous.
    ///
    /// Case: the backend confirms select-right; later the previously
    /// active pane is already gone when its inactive style would apply.
    #[test]
    fn active_pane_change_moves_focus_and_inactive_style() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<FocusedWebview>()
            .insert_resource(OrzmaConfigsResource::default())
            .add_observer(on_active_pane_changed);
        let a = app
            .world_mut()
            .spawn((OrzmaTerminal, OrzmuxPane(PaneId(1)), KeyboardFocused))
            .id();
        let b = app
            .world_mut()
            .spawn((OrzmaTerminal, OrzmuxPane(PaneId(2))))
            .id();
        app.world_mut().trigger(OrzmuxActivePaneChanged {
            previous: Some(a),
            current: Some(b),
        });
        app.update();
        assert!(app.world().get::<KeyboardFocused>(a).is_none());
        assert!(app.world().get::<KeyboardFocused>(b).is_some());
        assert!(app.world().get::<PaneInactiveStyle>(a).is_some());
        assert!(app.world().get::<PaneInactiveStyle>(b).is_none());

        app.world_mut().entity_mut(b).despawn();
        app.world_mut().trigger(OrzmuxActivePaneChanged {
            previous: Some(b),
            current: Some(a),
        });
        app.update();
        assert!(
            app.world().get::<KeyboardFocused>(a).is_some(),
            "a despawned previous must not panic"
        );
    }

    /// Asserts that a click on a pane asks the bridge to select it, and
    /// that a click on a non-pane entity asks nothing.
    ///
    /// Case: the user clicks an inactive pane, then clicks a webview
    /// child that is not itself a pane.
    #[test]
    fn a_click_requests_the_selection_of_a_pane_only() {
        #[derive(Resource, Default)]
        struct Seen(Vec<PaneAction>);

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<Seen>()
            .add_observer(on_pane_clicked)
            .add_observer(|ev: On<RequestPaneAction>, mut seen: ResMut<Seen>| {
                seen.0.push(ev.action)
            });
        let pane = app
            .world_mut()
            .spawn((OrzmaTerminal, OrzmuxPane(PaneId(2))))
            .id();
        let other = app.world_mut().spawn(OrzmaTerminal).id();
        app.world_mut().trigger(PaneClicked { entity: pane });
        app.world_mut().trigger(PaneClicked { entity: other });
        app.update();
        assert_eq!(
            app.world().resource::<Seen>().0,
            vec![PaneAction::Select(pane)]
        );
    }

    #[test]
    fn disables_input_on_any_guard() {
        assert!(!should_disable_input(false, true, false));
        assert!(should_disable_input(true, true, false));
        assert!(should_disable_input(false, false, false));
        assert!(should_disable_input(false, true, true));
    }

    /// Shell terminal (`OrzmaTerminal`) at window center (400,300),
    /// size 800x600, with one interactive inline rect rows 2..12, cols 3..43
    /// (phys y 32..192, x 24..344 at the 8x16 px cell pitch). Runs
    /// `maintain_input_gates`. Returns `(app, shell)`.
    fn make_gate_app() -> (App, Entity) {
        use bevy::math::IVec4;
        use bevy::window::WindowResolution;
        use bevy_orzma_tty_renderer::CellMetrics;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<ImeState>();
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
            phys_font_size: 16,
        });
        app.add_systems(Update, maintain_input_gates);

        let mut overlays = TerminalOverlays::default();
        overlays.rects[0] = IVec4::new(2, 3, 10, 40);
        let shell = app
            .world_mut()
            .spawn((
                OrzmaTerminal,
                ComputedNode {
                    size: Vec2::new(800.0, 600.0),
                    ..ComputedNode::DEFAULT
                },
                UiGlobalTransform::from_xy(400.0, 300.0),
                overlays,
            ))
            .id();
        app.world_mut().spawn((
            ChildOf(shell),
            Webview {
                handle: "w".into(),
                instance: InstanceId(1),
                slot: 0,
                rows: 10,
                cols: 40,
            },
        ));
        app.world_mut().spawn((
            Window {
                focused: true,
                resolution: WindowResolution::new(800, 600),
                ..default()
            },
            PrimaryWindow,
        ));
        (app, shell)
    }

    fn set_gate_cursor(app: &mut App, phys: Vec2) {
        use bevy::math::DVec2;
        let win = app
            .world_mut()
            .query_filtered::<Entity, With<PrimaryWindow>>()
            .single(app.world())
            .unwrap();
        app.world_mut()
            .get_mut::<Window>(win)
            .unwrap()
            .set_physical_cursor_position(Some(DVec2::new(phys.x as f64, phys.y as f64)));
    }

    #[test]
    fn cursor_over_webview_rect_disables_mouse() {
        let (mut app, shell) = make_gate_app();
        set_gate_cursor(&mut app, Vec2::new(40.0, 48.0));
        app.update();
        assert!(
            app.world().entity(shell).contains::<MouseDisabled>(),
            "the cursor over an interactive webview rect must MouseDisable the shell so \
             dispatch_mouse_buttons yields the click to the webview router"
        );
    }

    #[test]
    fn focused_webview_off_rect_keeps_mouse_enabled() {
        let (mut app, shell) = make_gate_app();
        let child = app
            .world_mut()
            .query_filtered::<Entity, With<Webview>>()
            .single(app.world())
            .unwrap();
        app.world_mut().resource_mut::<FocusedWebview>().0 = Some(child);
        set_gate_cursor(&mut app, Vec2::new(400.0, 400.0));
        app.update();
        assert!(
            !app.world().entity(shell).contains::<MouseDisabled>(),
            "webview focus alone must NOT MouseDisable the shell — an off-rect click must fall \
             through to the terminal (and clear webview focus in the router)"
        );
    }
}
