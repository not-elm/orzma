//! Tab intersurface: left-click a tab to focus its pane and switch to its
//! surface, plus a pointer cursor while hovering a tab. Mirrors the browser
//! toolbar's `Interaction`-driven pattern in `crate::browser_render`.

use crate::input::InputPhase;
use crate::ui::TabButton;
use bevy::prelude::*;
use bevy::window::{CursorIcon, PrimaryWindow, SystemCursorIcon};
use ozmux_multiplexer::{AttachedSession, MultiplexerCommands, SessionMarker};

/// Wires tab intersurface: `drive_tab_clicks` (click → focus pane + switch
/// surface) and `tab_hover_cursor` (pointer cursor on hover, after the hover
/// phase so it wins over the hyperlink system's `Text` write).
pub(crate) struct TabInteractionPlugin;

impl Plugin for TabInteractionPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                // NOTE: drive_tab_clicks mutates `Session::active_pane`, which
                // dispatch_focused_key (InputPhase::FocusedKey) reads to route
                // keystrokes, and which the chrome-rebuild / dim systems (run
                // after OzmuxSystems::Input) react to. It MUST live in
                // InputPhase::Dispatch so that focus change is visible the same
                // frame — see the OzmuxSystems::Input invariant in system_set.rs
                // and the matching placement of dispatch_mouse_buttons.
                drive_tab_clicks.in_set(InputPhase::Dispatch),
                tab_hover_cursor.after(InputPhase::Hover),
            ),
        );
    }
}

/// Routes a left-press on a tab to a focus + surface switch: focuses the tab's
/// pane (`set_active_pane`) and makes the tab's surface active
/// (`set_active_surface`). Mirrors `crate::browser_render::drive_nav_buttons`.
fn drive_tab_clicks(
    mut mux: MultiplexerCommands,
    tabs: Query<(&Interaction, &TabButton), Changed<Interaction>>,
    attached: Query<Entity, (With<SessionMarker>, With<AttachedSession>)>,
) {
    let attached_session = attached.single().ok();
    for (interaction, tab) in tabs.iter() {
        if *interaction != Interaction::Pressed {
            continue;
        }
        if let Some(session) = attached_session
            && let Err(e) = mux.set_active_pane(session, tab.pane)
        {
            tracing::warn!(target: "ozmux_gui::ui", ?e, "tab click: set_active_pane failed");
        }
        // NOTE: the surface switch is intentionally unconditional — a tab click
        // still selects its surface even if no session is attached (the pane
        // entity fully targets the switch); only the pane-focus step needs a
        // session. Do not gate this on `attached`.
        if let Err(e) = mux.set_active_surface(tab.pane, tab.surface) {
            tracing::warn!(target: "ozmux_gui::ui", ?e, "tab click: set_active_surface failed");
        }
    }
}

/// Shows a pointer cursor while the mouse hovers any tab, so tabs read as
/// clickable. Runs after `InputPhase::Hover` so it wins over the hyperlink
/// system's baseline cursor write; leaving a tab reverts to that baseline (the
/// I-beam over terminal text, the arrow over chrome) when the hyperlink system
/// re-asserts. Mirrors `crate::browser_render::nav_button_hover_cursor`.
fn tab_hover_cursor(
    tabs: Query<&Interaction, With<TabButton>>,
    mut cursor_icons: Query<&mut CursorIcon, With<PrimaryWindow>>,
) {
    let hovering = tabs
        .iter()
        .any(|i| matches!(i, Interaction::Hovered | Interaction::Pressed));
    if !hovering {
        return;
    }
    let Ok(mut icon) = cursor_icons.single_mut() else {
        return;
    };
    if !matches!(&*icon, CursorIcon::System(e) if *e == SystemCursorIcon::Pointer) {
        *icon = CursorIcon::System(SystemCursorIcon::Pointer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use ozmux_multiplexer::{
        ActivePane, ActiveSurface, AttachedSession, MultiplexerCommands, MultiplexerPlugin, Side,
        SplitOrientation, SurfaceKind,
    };

    /// Builds an app running `drive_tab_clicks`, with one attached session whose
    /// single pane has two surfaces — the first active, the second added but
    /// not activated. Returns `(app, session, pane, first_surface,
    /// second_surface)`.
    fn app_with_two_surfaces() -> (App, Entity, Entity, Entity, Entity) {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(MultiplexerPlugin);
        app.add_systems(Update, drive_tab_clicks);

        let (session, pane, first) = app
            .world_mut()
            .run_system_once(|mut mux: MultiplexerCommands| {
                let o = mux.create_session(Some("test".into()));
                (o.session, o.pane, o.surface)
            })
            .unwrap();
        app.world_mut().flush();

        // `add_surface` does NOT activate the new surface, so `first` stays
        // the active one.
        let second = app
            .world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.add_surface(pane, SurfaceKind::Terminal)
            })
            .unwrap();
        app.world_mut().flush();

        app.world_mut().entity_mut(session).insert(AttachedSession);
        (app, session, pane, first, second)
    }

    #[test]
    fn tab_press_focuses_pane_and_switches_surface() {
        let (mut app, session, pane, first, second) = app_with_two_surfaces();
        assert_eq!(
            app.world().get::<ActiveSurface>(pane).map(|a| a.0),
            Some(first),
            "precondition: the first surface is active before the click",
        );

        // A freshly-added Interaction::Pressed satisfies the Changed<Interaction>
        // filter.
        app.world_mut().spawn((
            TabButton {
                pane,
                surface: second,
            },
            Interaction::Pressed,
        ));
        app.update();

        assert_eq!(
            app.world().get::<ActiveSurface>(pane).map(|a| a.0),
            Some(second),
            "pressing a tab switches the pane's active surface",
        );
        assert_eq!(
            app.world().get::<ActivePane>(session).map(|a| a.0),
            Some(pane),
            "pressing a tab focuses its pane",
        );
    }

    #[test]
    fn tab_hovered_not_pressed_does_not_switch() {
        let (mut app, session, pane, first, second) = app_with_two_surfaces();

        app.world_mut().spawn((
            TabButton {
                pane,
                surface: second,
            },
            Interaction::Hovered,
        ));
        app.update();

        assert_eq!(
            app.world().get::<ActiveSurface>(pane).map(|a| a.0),
            Some(first),
            "a hovered (not pressed) tab must not switch the active surface",
        );
        assert_eq!(
            app.world().get::<ActivePane>(session).map(|a| a.0),
            Some(pane),
            "a hovered (not pressed) tab must not change the active pane",
        );
    }

    #[test]
    fn tab_press_in_unfocused_pane_focuses_that_pane() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(MultiplexerPlugin);
        app.add_systems(Update, drive_tab_clicks);

        let (session, original_pane) = app
            .world_mut()
            .run_system_once(|mut mux: MultiplexerCommands| {
                let o = mux.create_session(Some("test".into()));
                (o.session, o.pane)
            })
            .unwrap();
        app.world_mut().flush();

        // Split into a second pane, then re-focus the original so the new pane is
        // the unfocused one (`split_pane` promotes the new pane to active).
        let other_pane = app
            .world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                let p = mux
                    .split_pane(original_pane, Side::After, SplitOrientation::Horizontal)
                    .expect("split");
                mux.set_active_pane(session, original_pane)
                    .expect("refocus original");
                p
            })
            .unwrap();
        app.world_mut().flush();
        app.world_mut().entity_mut(session).insert(AttachedSession);

        let other_surface = app
            .world_mut()
            .run_system_once(move |mux: MultiplexerCommands| mux.panes_active_surface(other_pane))
            .unwrap()
            .expect("the split pane has an active surface");

        assert_eq!(
            app.world().get::<ActivePane>(session).map(|a| a.0),
            Some(original_pane),
            "precondition: the original pane is focused, the split pane is not",
        );

        // Click the already-active tab of the unfocused pane: the surface switch
        // is a no-op, but the pane focus must move to it in one click.
        app.world_mut().spawn((
            TabButton {
                pane: other_pane,
                surface: other_surface,
            },
            Interaction::Pressed,
        ));
        app.update();

        assert_eq!(
            app.world().get::<ActivePane>(session).map(|a| a.0),
            Some(other_pane),
            "clicking a tab in an unfocused pane focuses that pane",
        );
    }

    #[test]
    fn tab_hover_sets_pointer_cursor() {
        let mut world = World::new();
        let window = world
            .spawn((PrimaryWindow, CursorIcon::System(SystemCursorIcon::Text)))
            .id();
        let tab = world
            .spawn((
                TabButton {
                    pane: Entity::PLACEHOLDER,
                    surface: Entity::PLACEHOLDER,
                },
                Interaction::Hovered,
            ))
            .id();

        world.run_system_once(tab_hover_cursor).unwrap();
        assert!(
            matches!(
                world.get::<CursorIcon>(window),
                Some(CursorIcon::System(SystemCursorIcon::Pointer))
            ),
            "hovering a tab sets the pointer cursor",
        );

        // Not hovering: the system no-ops, leaving the cursor as the hover phase
        // set it (Text).
        *world.get_mut::<Interaction>(tab).unwrap() = Interaction::None;
        *world.get_mut::<CursorIcon>(window).unwrap() = CursorIcon::System(SystemCursorIcon::Text);
        world.run_system_once(tab_hover_cursor).unwrap();
        assert!(
            matches!(
                world.get::<CursorIcon>(window),
                Some(CursorIcon::System(SystemCursorIcon::Text))
            ),
            "no hovered tab leaves the cursor unchanged",
        );
    }
}
