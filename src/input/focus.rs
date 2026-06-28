//! Host-owned focus sync and input-suppression markers: keeps bevy_cef's
//! `FocusedWebview` in step with the active pane, and defines `KeyboardFocused`,
//! `KeyboardDisabled`, and `MouseDisabled` — the three components the host uses
//! to route keyboard and mouse input.

use crate::input::InputPhase;
use bevy::prelude::*;
use bevy_cef::prelude::{FocusedWebview, WebviewSource};
use ozma_terminal::OzmaTerminal;
use ozma_webview::{NonInteractive, Webview};

/// When present on an `OzmaTerminal` entity, the crate's default keyboard
/// dispatcher skips it entirely — the host routes keyboard input elsewhere
/// (tmux, a focused webview, an open picker, IME composition).
#[derive(Component)]
pub(crate) struct KeyboardDisabled;

/// When present on an `OzmaTerminal` entity, that terminal is the keyboard
/// focus: the crate's keyboard dispatcher routes raw keys to it, and the host
/// routes IME commits and anchors the OS candidate window to it. The host owns
/// focus policy and maintains the "exactly one focused" invariant; a terminal
/// with no `KeyboardFocused` receives no keyboard input.
#[derive(Component)]
pub(crate) struct KeyboardFocused;

/// When present on an `OzmaTerminal` entity, the host's mouse dispatchers and
/// hover-cursor system skip it — it is removed from the hit-test candidate set,
/// so the pointer falls through to the next terminal below it. The host marks
/// every terminal `MouseDisabled` for modal suppression (picker / IME / focused
/// webview / unfocused window).
#[derive(Component)]
pub(crate) struct MouseDisabled;

/// Registers the webview focus-sync system.
pub(crate) struct FocusSyncPlugin;

impl Plugin for FocusSyncPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, sync_focused_webview.after(InputPhase::FocusedKey));
    }
}

/// Keeps `bevy_cef`'s `FocusedWebview` in step with ozmux's active pane.
///
/// bevy_cef only updates `FocusedWebview` when a *webview* node is clicked
/// (`set_focus_on_press`), so moving focus to a terminal pane (a non-webview)
/// leaves the webview focused: its DOM text area keeps the caret and
/// `send_key_event` keeps routing keystrokes to it. Driving `FocusedWebview`
/// from the active pane fixes both — keyboard follows the focused pane, and CEF
/// blurs the webview on focus-leave (`bevy_cef`'s `apply_webview_focus` releases
/// CEF focus when `FocusedWebview` becomes `None`).
///
/// One case is PRESERVED instead of driven: when `FocusedWebview` holds an
/// webview child (`Webview`) whose `ChildOf` parent is a live
/// `OzmaTerminal` surface — active or not — that inline focus stands (spec §7, single
/// focus source). This covers click-granted focus and the app-declared focus
/// set via the control-plane `SetFocus` op, and means switching the active
/// pane does NOT clear a webview's focus: the webview keeps keyboard
/// focus until its child despawns (or focus moves off it), at which point the
/// sync falls through to the clear path below, which maps the active terminal
/// pane to `None`.
fn sync_focused_webview(
    mut focused: ResMut<FocusedWebview>,
    active_pane: Query<Entity, (With<OzmaTerminal>, With<KeyboardFocused>)>,
    webviews: Query<(), With<WebviewSource>>,
    non_interactive: Query<(), With<NonInteractive>>,
    webview_parents: Query<&ChildOf, With<Webview>>,
    surfaces: Query<(), With<OzmaTerminal>>,
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

#[cfg(test)]
mod tests {
    use super::*;

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

        // The active OzmaTerminal IS the active surface. The webview pane carries a
        // WebviewSource; the terminal pane does not.
        let terminal_pane = app.world_mut().spawn(OzmaTerminal).id();
        let ext_pane = app
            .world_mut()
            .spawn((OzmaTerminal, WebviewSource::new("ozma://memo/index.html")))
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

        // The active OzmaTerminal carries a NonInteractive WebviewSource: it must
        // never be focused.
        app.world_mut().spawn((
            OzmaTerminal,
            KeyboardFocused,
            WebviewSource::new("ozma://memo/index.html"),
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
    fn tmux_pane_inline_focus_is_preserved() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<FocusedWebview>();
        app.add_systems(Update, sync_focused_webview);

        let pane = app.world_mut().spawn(OzmaTerminal).id();
        let child = app
            .world_mut()
            .spawn((
                ChildOf(pane),
                Webview {
                    view_id: "v".into(),
                    instance_id: None,
                    slot: 0,
                },
            ))
            .id();
        app.world_mut().resource_mut::<FocusedWebview>().0 = Some(child);

        app.update();

        assert_eq!(
            app.world().resource::<FocusedWebview>().0,
            Some(child),
            "an inline child of a live OzmaTerminal must keep FocusedWebview across the per-frame sync",
        );
    }

    #[test]
    fn tmux_pane_inline_focus_is_gc_on_despawn() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<FocusedWebview>();
        app.add_systems(Update, sync_focused_webview);

        let pane = app.world_mut().spawn(OzmaTerminal).id();
        let child = app
            .world_mut()
            .spawn((
                ChildOf(pane),
                Webview {
                    view_id: "v".into(),
                    instance_id: None,
                    slot: 0,
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
        // NOTE: ozma_webview's apply_control_events and its supporting resource types
        // (OzmaRegistry, ControlEvents, etc.) are pub(crate) and unreachable from the
        // binary. Setting FocusedWebview directly produces the same world state that
        // apply_control_events(SetFocus) would — the sync behavior under test is
        // identical regardless of how FocusedWebview was last written.

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<FocusedWebview>()
            .add_systems(Update, sync_focused_webview);

        let surface = app.world_mut().spawn(OzmaTerminal).id();
        let child = app
            .world_mut()
            .spawn((
                ChildOf(surface),
                Webview {
                    view_id: "h1".into(),
                    instance_id: None,
                    slot: 0,
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
}
