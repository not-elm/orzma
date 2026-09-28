//! Focus and input suppression: gates which pane receives keyboard and
//! mouse input, and moves keyboard focus with the active pane.

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
use bevy_cef::prelude::FocusedWebview;
use bevy_orzma_tty_renderer::prelude::{
    PaneInactiveStyle, TerminalCellMetricsResource, TerminalOverlays,
};
use bevy_orzma_webview::{NonInteractive, RequestWebviewFocus, Webview, webview_hit_at};
use bevy_orzmux::prelude::{
    OrzmuxActivePaneChanged, OrzmuxPane, OrzmuxPaneHidden, PaneAction, RequestPaneAction,
};
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
/// hover-cursor system drop it from their hit-test candidate set, so the
/// pointer falls through to the next terminal below it. The host marks a
/// terminal `TerminalMouseDisabled` for IME composition or an unfocused
/// window.
#[derive(Component)]
pub(crate) struct TerminalMouseDisabled;

/// When present on an `OrzmaTerminal` entity, the webview router declines to
/// forward pointer input to its inline children. The host marks a terminal
/// `WebviewMouseDisabled` for vi mode, an unfocused window, or an IME
/// composition that no inline webview owns. A composition owned by a focused
/// inline webview is not a reason, and the page stays clickable throughout it.
#[derive(Component)]
pub(crate) struct WebviewMouseDisabled;

/// When present on an `OrzmaTerminal` entity, the cursor is over one of its
/// interactive inline webview rects. The host's mouse dispatchers and
/// hover-cursor system skip it for a new press and for hover, though a
/// gesture already held in it keeps reaching it, and the webview router
/// still acts on it. It is never set while the terminal is in vi mode.
#[derive(Component)]
pub(crate) struct MouseClaimedByWebview;

/// A press landed on a pane surface.
#[derive(EntityEvent, Debug, Clone, Copy)]
pub(crate) struct PaneClicked {
    #[event_target]
    pub entity: Entity,
}

/// Keeps focus and input gating in sync with the active pane and
/// click-to-focus requests.
pub(super) struct FocusSyncPlugin;

impl Plugin for FocusSyncPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, maintain_input_gates.before(InputPhase::Hover))
            .add_observer(on_active_pane_changed)
            .add_observer(on_pane_clicked);
    }
}

/// Inline-webview hit-test inputs for the mouse rect-claim. `metrics` is
/// optional: the gate still runs before cell metrics exist, when no claim
/// is possible yet.
#[derive(SystemParam)]
pub(in crate::input) struct WebviewClaimParams<'w, 's> {
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

/// Brings every `OrzmaTerminal`'s input gates up to date with the window
/// focus, the IME composition, vi mode, and the inline webview rect under
/// the cursor: inserts or removes `KeyboardDisabled`,
/// `TerminalMouseDisabled`, `WebviewMouseDisabled`, and
/// `MouseClaimedByWebview` so each marker is present exactly while its
/// condition holds. The markers apply at the next command flush. A pane
/// hidden with its workspace gets both mouse gates and never claims the
/// mouse for a webview.
pub(in crate::input) fn maintain_input_gates(
    mut commands: Commands,
    ime: Res<ImeState>,
    focused_webview: Res<FocusedWebview>,
    windows: Query<&Window, With<PrimaryWindow>>,
    terminals: Query<
        (
            Entity,
            Has<KeyboardDisabled>,
            Has<TerminalMouseDisabled>,
            Has<WebviewMouseDisabled>,
            Has<MouseClaimedByWebview>,
            Has<ViModeState>,
            Has<OrzmuxPaneHidden>,
        ),
        With<OrzmaTerminal>,
    >,
    claim: WebviewClaimParams,
) {
    let window = windows.single().ok();
    let focused = window.map(|w| w.focused).unwrap_or(false);
    let keyboard_disable =
        should_disable_input(ime.is_composing(), focused, focused_webview.0.is_some());
    // NOTE: webview focus alone claims nothing — only the cursor sitting over an
    // interactive inline rect does — so an off-rect click still reaches
    // `dispatch_mouse_buttons` and clears webview focus in the router. Folding
    // `focused_webview.0.is_some()` into either gate unconditionally would swallow
    // that fallthrough click, stranding the user on a focused webview. Both
    // conditional folds below are safe only because neither can be live while an
    // inline webview still holds focus: `webview_modal` adds the composing case
    // only while the composition has no owner, and `handle_enter_vi_mode_request`
    // releases the focused webview before `in_vi_mode` suppresses the webview gate.
    let mouse_modal = ime.is_composing() || !focused;
    let webview_modal = !focused || (ime.is_composing() && focused_webview.0.is_none());
    let claimed = window.and_then(|w| cursor_claims_webview(w, &claim));
    for (entity, has_keyboard, has_terminal, has_webview, has_claim, in_vi_mode, hidden) in
        terminals.iter()
    {
        set_marker(
            &mut commands,
            entity,
            KeyboardDisabled,
            keyboard_disable || in_vi_mode,
            has_keyboard,
        );
        set_marker(
            &mut commands,
            entity,
            TerminalMouseDisabled,
            mouse_modal || hidden,
            has_terminal,
        );
        set_marker(
            &mut commands,
            entity,
            WebviewMouseDisabled,
            webview_modal || in_vi_mode || hidden,
            has_webview,
        );
        set_marker(
            &mut commands,
            entity,
            MouseClaimedByWebview,
            Some(entity) == claimed && !in_vi_mode && !hidden,
            has_claim,
        );
    }
}

/// Applies an applied active-pane change, whether accepted from a
/// `Layout` or taken optimistically from a pane selection: moves
/// `KeyboardFocused` and swaps `PaneInactiveStyle` between the previous and
/// current panes. `previous` may already be despawned by a `PaneClosed` in
/// the same drain.
fn on_active_pane_changed(
    ev: On<OrzmuxActivePaneChanged>,
    mut commands: Commands,
    configs: Res<OrzmaConfigsResource>,
    focused: Query<Entity, With<KeyboardFocused>>,
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
}

/// Click-to-focus: asks the bridge to select the clicked pane, and asks for
/// the release of a webview focus held in any other pane. The bridge
/// applies the selection as active at once and reports the change through
/// `OrzmuxActivePaneChanged`, so this frame's keys already go to the
/// clicked pane; the confirming `Layout` reconciles.
fn on_pane_clicked(
    ev: On<PaneClicked>,
    mut commands: Commands,
    focused_webview: Res<FocusedWebview>,
    webview_parents: Query<&ChildOf, With<Webview>>,
    panes: Query<(), With<OrzmuxPane>>,
) {
    if panes.get(ev.entity).is_err() {
        return;
    }
    let focused_elsewhere = focused_webview
        .0
        .and_then(|webview| webview_parents.get(webview).ok())
        .is_some_and(|parent| parent.parent() != ev.entity);
    if focused_elsewhere {
        commands.trigger(RequestWebviewFocus::new(None));
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

/// Returns `true` when host-side keyboard input should be suppressed: IME
/// composing, window not focused, or a webview owns the keyboard.
fn should_disable_input(composing: bool, window_focused: bool, webview_focused: bool) -> bool {
    composing || !window_focused || webview_focused
}

/// The shell surface whose INTERACTIVE inline webview rect is under the
/// cursor, or `None`. Considers only the topmost surface under the cursor;
/// a `NonInteractive` child never claims it.
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

fn set_marker<C: Component>(
    commands: &mut Commands,
    entity: Entity,
    marker: C,
    want: bool,
    has: bool,
) {
    if want && !has {
        commands.entity(entity).insert(marker);
    } else if !want && has {
        commands.entity(entity).remove::<C>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orzma_vt::prelude::InstanceId;
    use orzma_webview_host::prelude::MountId;
    use orzmux::prelude::PaneId;

    /// Asserts that an accepted active change moves `KeyboardFocused`,
    /// tints the previous pane, leaves `FocusedWebview` on the webview it
    /// names, and tolerates a despawned previous.
    ///
    /// Case: the user switches to the right pane with a leader-key shortcut
    /// while a page in the left pane holds keyboard focus, and the backend
    /// confirms the switch. Later, the previously active pane is already
    /// gone when its inactive style would apply.
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
        let page = app
            .world_mut()
            .spawn((
                ChildOf(a),
                Webview::new("w".into(), InstanceId(1), MountId::new(1), 0, 10, 40),
            ))
            .id();
        app.world_mut().resource_mut::<FocusedWebview>().0 = Some(page);
        app.world_mut().trigger(OrzmuxActivePaneChanged {
            previous: Some(a),
            current: Some(b),
        });
        app.update();
        assert!(app.world().get::<KeyboardFocused>(a).is_none());
        assert!(app.world().get::<KeyboardFocused>(b).is_some());
        assert!(app.world().get::<PaneInactiveStyle>(a).is_some());
        assert!(app.world().get::<PaneInactiveStyle>(b).is_none());
        assert_eq!(app.world().resource::<FocusedWebview>().0, Some(page));

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
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<FocusedWebview>()
            .init_resource::<SelectRequests>()
            .add_observer(on_pane_clicked)
            .add_observer(record_select_request);
        let pane = app
            .world_mut()
            .spawn((OrzmaTerminal, OrzmuxPane(PaneId(2))))
            .id();
        let other = app.world_mut().spawn(OrzmaTerminal).id();
        app.world_mut().trigger(PaneClicked { entity: pane });
        app.world_mut().trigger(PaneClicked { entity: other });
        app.update();
        assert_eq!(
            app.world().resource::<SelectRequests>().0,
            vec![PaneAction::Select(pane)]
        );
    }

    /// Asserts that a click on a pane other than the one holding the focused
    /// webview asks for that focus to be released, while a click on the
    /// webview's own pane does not, and that both select their pane.
    ///
    /// Case: a page in the left pane holds the keyboard, and the user
    /// right-clicks the shell text in the left pane and then in the right
    /// pane.
    #[test]
    fn a_click_on_another_pane_releases_a_focused_webview() {
        #[derive(Resource, Default)]
        struct FocusRequests(Vec<Option<Entity>>);
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<FocusedWebview>()
            .init_resource::<SelectRequests>()
            .init_resource::<FocusRequests>()
            .add_observer(on_pane_clicked)
            .add_observer(record_select_request)
            .add_observer(
                |ev: On<RequestWebviewFocus>, mut requests: ResMut<FocusRequests>| {
                    requests.0.push(ev.target());
                },
            );
        let left = app
            .world_mut()
            .spawn((OrzmaTerminal, OrzmuxPane(PaneId(1))))
            .id();
        let right = app
            .world_mut()
            .spawn((OrzmaTerminal, OrzmuxPane(PaneId(2))))
            .id();
        let page = app
            .world_mut()
            .spawn((
                ChildOf(left),
                Webview::new("w".into(), InstanceId(1), MountId::new(1), 0, 10, 40),
            ))
            .id();
        app.world_mut().resource_mut::<FocusedWebview>().0 = Some(page);
        app.world_mut().trigger(PaneClicked { entity: left });
        app.world_mut().trigger(PaneClicked { entity: right });
        app.update();
        assert_eq!(app.world().resource::<FocusRequests>().0, vec![None]);
        assert_eq!(
            app.world().resource::<SelectRequests>().0,
            vec![PaneAction::Select(left), PaneAction::Select(right)]
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
    /// `maintain_input_gates` under the production `InputPhase` ordering.
    /// Returns `(app, shell)`.
    fn make_gate_app() -> (App, Entity) {
        use bevy::math::IVec4;
        use bevy::window::WindowResolution;
        use bevy_orzma_tty_renderer::prelude::CellMetrics;

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
        app.configure_sets(
            Update,
            (
                InputPhase::Hover,
                InputPhase::Dispatch,
                InputPhase::FocusedKey,
            )
                .chain(),
        );
        app.add_systems(Update, maintain_input_gates.before(InputPhase::Hover));

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
            Webview::new("w".into(), InstanceId(1), MountId::new(1), 0, 10, 40),
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

    /// Asserts that the cursor over an interactive webview rect claims the
    /// shell for the webview without suppressing its mouse input.
    ///
    /// Case: the user moves the pointer onto a page mounted in a pane.
    #[test]
    fn cursor_over_webview_rect_claims_the_shell() {
        let (mut app, shell) = make_gate_app();
        set_gate_cursor(&mut app, Vec2::new(40.0, 48.0));
        app.update();
        assert!(
            app.world()
                .entity(shell)
                .contains::<MouseClaimedByWebview>(),
            "the rect-claim marks the shell so the terminal dispatchers yield to the router"
        );
        assert!(
            !app.world()
                .entity(shell)
                .contains::<TerminalMouseDisabled>(),
            "the claim must not suppress the shell — the router is kept out by \
             WebviewMouseDisabled, not by the claim"
        );
    }

    /// Asserts that webview focus alone claims nothing and suppresses
    /// nothing while the cursor sits outside every rect.
    ///
    /// Case: a page is focused in a pane and the user clicks on the
    /// terminal text beside it.
    #[test]
    fn focused_webview_off_rect_claims_nothing() {
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
            !app.world()
                .entity(shell)
                .contains::<MouseClaimedByWebview>(),
            "an off-rect cursor claims nothing, so the press falls through to the terminal"
        );
        assert!(
            !app.world()
                .entity(shell)
                .contains::<TerminalMouseDisabled>(),
            "webview focus alone must not suppress the shell"
        );
    }

    /// Builds an `ImeState` holding a live preedit and inserts it, replacing
    /// the non-composing default `make_gate_app` installed.
    fn set_composing(app: &mut App) {
        use crate::input::ime::apply_event;
        use bevy::window::Ime;

        let mut state = ImeState::default();
        apply_event(
            &mut state,
            &Ime::Preedit {
                window: Entity::PLACEHOLDER,
                value: "あ".into(),
                cursor: Some((3, 3)),
            },
        );
        assert!(
            state.is_composing(),
            "the fixture must actually compose, or the gate assertions below are vacuous"
        );
        app.insert_resource(state);
    }

    /// Asserts that a composition owned by a focused inline webview sets the
    /// terminal gate and leaves the webview gate clear.
    ///
    /// Case: the user is typing Japanese into a text field on a page mounted
    /// in a pane, and moves the pointer over that page mid-conversion.
    #[test]
    fn a_webview_owned_composition_leaves_the_webview_gate_clear() {
        let (mut app, shell) = make_gate_app();
        let child = app
            .world_mut()
            .query_filtered::<Entity, With<Webview>>()
            .single(app.world())
            .unwrap();
        app.world_mut().resource_mut::<FocusedWebview>().0 = Some(child);
        set_composing(&mut app);
        set_gate_cursor(&mut app, Vec2::new(400.0, 400.0));
        app.update();
        assert!(
            app.world()
                .entity(shell)
                .contains::<TerminalMouseDisabled>(),
            "a composition still suppresses the terminal's own mouse input"
        );
        assert!(
            !app.world().entity(shell).contains::<WebviewMouseDisabled>(),
            "the page owning the composition must stay clickable while it composes"
        );
    }

    /// Asserts that a composition no inline webview owns sets both gates.
    ///
    /// Case: the user is typing Japanese at the shell prompt while a page is
    /// mounted in the same pane but not focused.
    #[test]
    fn an_unowned_composition_sets_both_gates() {
        let (mut app, shell) = make_gate_app();
        set_composing(&mut app);
        set_gate_cursor(&mut app, Vec2::new(400.0, 400.0));
        app.update();
        assert!(
            app.world()
                .entity(shell)
                .contains::<TerminalMouseDisabled>(),
            "a composition suppresses the terminal's own mouse input"
        );
        assert!(
            app.world().entity(shell).contains::<WebviewMouseDisabled>(),
            "a stray click into a page mid-preedit would discard the pending commit, \
             so an unowned composition suppresses the page too"
        );
    }

    /// Asserts that vi mode sets the webview gate but leaves the terminal's
    /// own mouse input enabled.
    ///
    /// Case: the user enters vi mode to select text with the mouse in a pane
    /// that has a page mounted in it.
    #[test]
    fn vi_mode_sets_only_the_webview_gate() {
        let (mut app, shell) = make_gate_app();
        app.world_mut().entity_mut(shell).insert(ViModeState);
        set_gate_cursor(&mut app, Vec2::new(400.0, 400.0));
        app.update();
        assert!(
            !app.world()
                .entity(shell)
                .contains::<TerminalMouseDisabled>(),
            "vi mode keeps terminal mouse selection and the wheel working"
        );
        assert!(
            app.world().entity(shell).contains::<WebviewMouseDisabled>(),
            "vi mode must reach the page too, or a click in vi mode still drives it"
        );
    }

    /// Asserts that a webview rect under the cursor does not claim the
    /// pointer while the terminal is in vi mode.
    ///
    /// Case: in vi mode the user starts a drag over a mounted page to select
    /// the terminal text around it.
    #[test]
    fn vi_mode_keeps_a_webview_rect_from_claiming_the_pointer() {
        let (mut app, shell) = make_gate_app();
        app.world_mut().entity_mut(shell).insert(ViModeState);
        set_gate_cursor(&mut app, Vec2::new(40.0, 48.0));
        app.update();
        assert!(
            !app.world()
                .entity(shell)
                .contains::<MouseClaimedByWebview>()
        );
    }

    /// Asserts that a hidden pane gets both mouse gates and stops claiming
    /// the mouse for a webview, so a gesture held in it is cancelled and a
    /// pressed webview in it is released.
    ///
    /// Case: the user presses the switch-workspace key while dragging a
    /// selection over a page mounted in a pane.
    #[test]
    fn a_hidden_pane_gets_both_mouse_gates() {
        let (mut app, shell) = make_gate_app();
        set_gate_cursor(&mut app, Vec2::new(40.0, 48.0));
        app.update();
        assert!(
            app.world()
                .entity(shell)
                .contains::<MouseClaimedByWebview>(),
            "the cursor over the rect must claim the shell before the pane is hidden, or the \
             later absence proves nothing"
        );
        app.world_mut().entity_mut(shell).insert(OrzmuxPaneHidden);
        app.update();
        assert!(
            app.world()
                .entity(shell)
                .contains::<TerminalMouseDisabled>()
        );
        assert!(app.world().entity(shell).contains::<WebviewMouseDisabled>());
        assert!(
            !app.world()
                .entity(shell)
                .contains::<MouseClaimedByWebview>()
        );
        app.world_mut()
            .entity_mut(shell)
            .remove::<OrzmuxPaneHidden>();
        app.update();
        assert!(
            !app.world()
                .entity(shell)
                .contains::<TerminalMouseDisabled>()
        );
        assert!(!app.world().entity(shell).contains::<WebviewMouseDisabled>());
    }

    #[derive(Resource, Default)]
    struct SelectRequests(Vec<PaneAction>);

    fn record_select_request(ev: On<RequestPaneAction>, mut seen: ResMut<SelectRequests>) {
        seen.0.push(ev.action);
    }

    /// Asserts that the rect-claim `maintain_input_gates` writes is visible
    /// to a system in `InputPhase::Dispatch` within the same update.
    ///
    /// Case: the host gates a terminal on the frame the pointer reaches an
    /// interactive rect, and the dispatchers have to see that gate on the
    /// same press.
    #[test]
    fn a_gate_inserted_before_hover_is_visible_in_dispatch() {
        #[derive(Resource, Default)]
        struct SawMarker(bool);

        let (mut app, shell) = make_gate_app();
        app.init_resource::<SawMarker>().add_systems(
            Update,
            (move |mut saw: ResMut<SawMarker>, gated: Query<Has<MouseClaimedByWebview>>| {
                if let Ok(has) = gated.get(shell) {
                    saw.0 = has;
                }
            })
            .in_set(InputPhase::Dispatch),
        );
        set_gate_cursor(&mut app, Vec2::new(40.0, 48.0));
        app.update();
        assert!(
            app.world().resource::<SawMarker>().0,
            "the sync point at the ordering edge applies the claim before Dispatch runs"
        );
    }
}
