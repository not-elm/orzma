//! Pane augmentation and inactive-pane styling: gives each multiplexer pane a
//! `Button` click target with `FocusPolicy::Block` (load-bearing: stops pane
//! clicks reaching webview surfaces), and tints the background of (and
//! optionally dims) every inactive pane at the renderer via
//! `PaneInactiveStyle` (the terminal shader blends inactive backgrounds
//! toward a configured grey). There is no literal drawn border. Keyboard
//! focus sync onto the active pane lives in `crate::multiplexer::window`
//! (`sync_keyboard_focus_to_active_pane`), not here.

use crate::configs::OrzmaConfigsResource;
use crate::multiplexer::pane::MultiplexerPane;
use crate::multiplexer::window::{ActiveMultiplexerWindow, MultiplexerWindow};
use bevy::prelude::*;
use bevy::ui::FocusPolicy;
use orzma_tty_engine::TerminalHandle;
use orzma_tty_renderer::material::PaneInactiveStyle;

/// Registers the pane augmentation (`Button` + `FocusPolicy::Block`) and
/// inactive-pane style sync systems.
pub(super) struct PaneFocusPlugin;

impl Plugin for PaneFocusPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                augment_multiplexer_pane,
                sync_inactive_pane_style.run_if(pane_style_needs_sync),
            ),
        );
    }
}

/// Gives each rendered pane (one that has its `TerminalHandle` but no
/// `Button` yet) a `Button` click target with an explicit
/// `FocusPolicy::Block`. The `Without<Button>` filter makes this run exactly
/// once per pane.
///
/// `FocusPolicy::Block` is provided explicitly because `Button`'s required
/// `FocusPolicy::Block` is silently skipped when the pane already carries
/// `FocusPolicy::Pass` (from `Node`'s required-component default). An
/// explicit insert in the bundle replaces the existing component.
fn augment_multiplexer_pane(
    mut commands: Commands,
    panes: Query<Entity, (With<MultiplexerPane>, With<TerminalHandle>, Without<Button>)>,
) {
    for pane in panes.iter() {
        commands.entity(pane).insert((Button, FocusPolicy::Block));
    }
}

/// True when a pane's inactive style may need re-syncing this frame: a new
/// pane appeared (needs its initial style), or the active window's
/// `active_pane` changed.
fn pane_style_needs_sync(
    added_panes: Query<(), Added<MultiplexerPane>>,
    changed_active: Query<(), (With<ActiveMultiplexerWindow>, Changed<MultiplexerWindow>)>,
) -> bool {
    !added_panes.is_empty() || !changed_active.is_empty()
}

/// Sets each pane's [`PaneInactiveStyle`] from the active window's
/// `active_pane`: the active pane (or every pane, when no window is active)
/// has the component REMOVED — the renderer treats an absent
/// `PaneInactiveStyle` as the active / no-op style — while every other pane
/// gets the configured inactive style inserted. Only inserts/removes when
/// the current state differs from the wanted one. Gated by
/// `pane_style_needs_sync`, so it does not observe live config edits — a
/// config reload re-applies on the next active-pane change.
fn sync_inactive_pane_style(
    mut commands: Commands,
    active_windows: Query<&MultiplexerWindow, With<ActiveMultiplexerWindow>>,
    panes: Query<(Entity, Option<&PaneInactiveStyle>), With<MultiplexerPane>>,
    configs: Option<Res<OrzmaConfigsResource>>,
) {
    let active_pane = active_windows
        .single()
        .ok()
        .map(|window| window.active_pane);
    let inactive = inactive_style(configs.as_deref());
    for (entity, current) in panes.iter() {
        let is_active = active_pane.is_none() || active_pane == Some(entity);
        if is_active {
            if current.is_some() {
                commands.entity(entity).remove::<PaneInactiveStyle>();
            }
        } else if current.copied() != Some(inactive) {
            commands.entity(entity).insert(inactive);
        }
    }
}

/// Returns the [`PaneInactiveStyle`] applied to inactive panes when the
/// treatment is enabled: background tint (`tint_color` linearized + `tint`
/// amount) and the webview overlay treatment (`webview_dim` /
/// `webview_desaturate`). Disabled or absent config yields the no-op default.
fn inactive_style(configs: Option<&OrzmaConfigsResource>) -> PaneInactiveStyle {
    match configs {
        Some(cfg) if cfg.0.inactive_pane.enabled => {
            let (r, g, b) = cfg.0.inactive_pane.tint_color_rgb();
            let lin = Color::srgb_u8(r, g, b).to_linear();
            PaneInactiveStyle {
                dim: cfg.0.inactive_pane.dim,
                tint: Vec4::new(lin.red, lin.green, lin.blue, cfg.0.inactive_pane.tint),
                overlay_dim: cfg.0.inactive_pane.webview_dim,
                overlay_desaturate: cfg.0.inactive_pane.webview_desaturate,
            }
        }
        _ => PaneInactiveStyle::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spawns an active window (`MultiplexerWindow` + `ActiveMultiplexerWindow`)
    /// whose `active_pane` starts on `pane_a`, plus two `MultiplexerPane`
    /// entities (each carrying a detached `TerminalHandle`) belonging to it.
    /// Mirrors `window::tests::spawn_two_pane_vertical_window`, minus the
    /// layout tree this module doesn't need. Returns `(window, pane_a, pane_b)`.
    fn spawn_active_window_with_two_panes(app: &mut App) -> (Entity, Entity, Entity) {
        let world = app.world_mut();
        let window = world.spawn_empty().id();
        let pane_a = world
            .spawn((MultiplexerPane { window }, TerminalHandle::detached(10, 5)))
            .id();
        let pane_b = world
            .spawn((MultiplexerPane { window }, TerminalHandle::detached(10, 5)))
            .id();
        world.entity_mut(window).insert((
            MultiplexerWindow {
                index: 0,
                name: None,
                active_pane: pane_a,
            },
            ActiveMultiplexerWindow,
        ));
        (window, pane_a, pane_b)
    }

    #[test]
    fn augment_adds_button_and_focus_block() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, PaneFocusPlugin));
        let window = app.world_mut().spawn_empty().id();
        let pane = app
            .world_mut()
            .spawn((MultiplexerPane { window }, TerminalHandle::detached(10, 5)))
            .id();

        app.update();

        assert!(
            app.world().get::<Button>(pane).is_some(),
            "pane gains a Button"
        );
        assert_eq!(
            app.world().get::<FocusPolicy>(pane).copied(),
            Some(FocusPolicy::Block),
            "pane gets FocusPolicy::Block to capture clicks",
        );
        let children = app
            .world()
            .get::<Children>(pane)
            .map(|c| c.len())
            .unwrap_or(0);
        assert_eq!(children, 0, "no overlay child spawned");

        app.update();
        let children_after = app
            .world()
            .get::<Children>(pane)
            .map(|c| c.len())
            .unwrap_or(0);
        assert_eq!(children_after, 0, "augment runs exactly once per pane");
    }

    #[test]
    fn active_pane_has_no_inactive_style_others_do() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, PaneFocusPlugin));
        app.insert_resource(OrzmaConfigsResource::default());
        let (window, p1, p2) = spawn_active_window_with_two_panes(&mut app);

        let style = |app: &App, e| app.world().get::<PaneInactiveStyle>(e).copied();
        // Expected inactive value from the default config (#3a3b45 @ 0.85,
        // dim 1.0, webview 0.55/0.6), built the same way as `inactive_style`.
        let lin = Color::srgb_u8(0x3a, 0x3b, 0x45).to_linear();
        let inactive = PaneInactiveStyle {
            dim: 1.0,
            tint: Vec4::new(lin.red, lin.green, lin.blue, 0.85),
            overlay_dim: 0.55,
            overlay_desaturate: 0.6,
        };

        app.update();
        assert_eq!(
            style(&app, p1),
            None,
            "active pane carries no PaneInactiveStyle"
        );
        assert_eq!(
            style(&app, p2),
            Some(inactive),
            "inactive pane gets the configured style"
        );

        app.world_mut()
            .get_mut::<MultiplexerWindow>(window)
            .unwrap()
            .active_pane = p2;
        app.update();

        assert_eq!(
            style(&app, p2),
            None,
            "the new active pane loses its PaneInactiveStyle"
        );
        assert_eq!(
            style(&app, p1),
            Some(inactive),
            "the former active pane gains the configured style"
        );
    }

    #[test]
    fn disabled_config_leaves_inactive_pane_untreated() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, PaneFocusPlugin));
        let mut configs = OrzmaConfigsResource::default();
        configs.0.inactive_pane.enabled = false;
        app.insert_resource(configs);
        let (_window, p1, p2) = spawn_active_window_with_two_panes(&mut app);

        app.update();

        let style = |app: &App, e| app.world().get::<PaneInactiveStyle>(e).copied();
        assert_eq!(
            style(&app, p1),
            None,
            "active pane carries no PaneInactiveStyle"
        );
        assert_eq!(
            style(&app, p2),
            Some(PaneInactiveStyle::default()),
            "enabled=false: inactive pane gets only the no-op default style"
        );
    }
}
