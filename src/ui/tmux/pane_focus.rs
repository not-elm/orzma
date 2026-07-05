//! Pane augmentation and inactive-pane styling: gives each tmux pane a `Button`
//! click target with `FocusPolicy::Block` (load-bearing: stops pane clicks
//! reaching webview surfaces), and tints the background of (and optionally dims)
//! every inactive pane at the renderer via `PaneInactiveStyle` (the terminal
//! shader blends inactive backgrounds toward a configured grey). `select-pane`
//! on press is owned by the tmux mouse gesture system
//! (`mouse::MousePlugin`).

use crate::app_mode::TmuxActiveSet;
use crate::configs::OzmuxConfigsResource;
use crate::input::InputPhase;
use crate::input::focus::KeyboardFocused;
use bevy::prelude::*;
use bevy::ui::FocusPolicy;
use ozma_tty_engine::TerminalHandle;
use ozma_tty_renderer::material::PaneInactiveStyle;
use ozmux_tmux::{ActivePane, TmuxPane, TmuxProjectionSet};

/// Registers the pane augmentation (adds `Button` + `FocusPolicy::Block`) and
/// dim systems. `select-pane` on press is handled by `tmux_gesture`.
pub(super) struct PaneFocusPlugin;

impl Plugin for PaneFocusPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                augment_tmux_pane.after(TmuxProjectionSet),
                sync_inactive_pane_style.run_if(pane_active_state_changed),
                // NOTE: both edges are load-bearing. `.after(TmuxProjectionSet)`
                // runs the mirror once `ActivePane` is fresh; `.before(FocusedKey)`
                // flushes the deferred `KeyboardFocused` insert before
                // `resolve_shortcuts` reads it, so `batch.focused` reflects the
                // current active pane the same frame it changes.
                sync_keyboard_focus_to_active_pane
                    .run_if(pane_active_state_changed)
                    .after(TmuxProjectionSet)
                    .before(InputPhase::FocusedKey),
            )
                .in_set(TmuxActiveSet),
        );
    }
}

/// Gives each rendered pane (one that has its `TerminalHandle` but no `Button`
/// yet) a `Button` click target with an explicit `FocusPolicy::Block`. The
/// `Without<Button>` filter makes this run exactly once per pane.
///
/// `FocusPolicy::Block` is provided explicitly because `Button`'s required
/// `FocusPolicy::Block` is silently skipped when the pane already carries
/// `FocusPolicy::Pass` (from `Node`'s required-component default). An explicit
/// insert in the bundle replaces the existing component.
fn augment_tmux_pane(
    mut commands: Commands,
    panes: Query<Entity, (With<TmuxPane>, With<TerminalHandle>, Without<Button>)>,
) {
    for pane in panes.iter() {
        commands.entity(pane).insert((Button, FocusPolicy::Block));
    }
}

/// True when a pane's active state may have changed this frame: a new pane
/// appeared, or the `ActivePane` marker was inserted/removed.
fn pane_active_state_changed(
    mut removed_active: RemovedComponents<ActivePane>,
    added_panes: Query<(), Added<TmuxPane>>,
    added_active: Query<(), Added<ActivePane>>,
) -> bool {
    added_panes.iter().next().is_some()
        || added_active.iter().next().is_some()
        || removed_active.read().next().is_some()
}

/// Sets each pane's [`PaneInactiveStyle`] on the `TmuxPane` entity (which owns
/// the `TerminalGrid` / material): the active pane (or every pane when none is
/// active) gets the default no-op; inactive panes get the configured style
/// (background tint + overlay dim/desaturate). Only inserts when the value
/// changes. Gated by `pane_active_state_changed`, so it does not observe live
/// config edits — a config reload re-applies on the next active-pane change.
fn sync_inactive_pane_style(
    mut commands: Commands,
    panes: Query<(Entity, Has<ActivePane>, Option<&PaneInactiveStyle>), With<TmuxPane>>,
    configs: Option<Res<OzmuxConfigsResource>>,
) {
    let inactive = inactive_style(configs.as_deref());
    let any_active = panes.iter().any(|(_, active, _)| active);
    for (entity, active, current) in panes.iter() {
        let want = if active || !any_active {
            PaneInactiveStyle::default()
        } else {
            inactive
        };
        if current.copied() != Some(want) {
            commands.entity(entity).insert(want);
        }
    }
}

/// Mirrors tmux's `ActivePane` onto the terminal-level `KeyboardFocused` marker:
/// the active pane gains `KeyboardFocused`, every other pane loses it. This is
/// the host's bridge from the multiplexer's active-pane notion to the terminal
/// crate's single focus marker (which `ozma_webview` and IME/title read).
/// Writes conditionally so change detection fires only on real changes.
fn sync_keyboard_focus_to_active_pane(
    mut commands: Commands,
    panes: Query<(Entity, Has<ActivePane>, Has<KeyboardFocused>), With<TmuxPane>>,
) {
    for (entity, active, focused) in panes.iter() {
        if active && !focused {
            commands.entity(entity).insert(KeyboardFocused);
        } else if !active && focused {
            commands.entity(entity).remove::<KeyboardFocused>();
        }
    }
}

/// Returns the [`PaneInactiveStyle`] applied to inactive panes when the
/// treatment is enabled: background tint (`tint_color` linearized + `tint`
/// amount) and the webview overlay treatment (`webview_dim` /
/// `webview_desaturate`). Disabled or absent config yields the no-op default.
fn inactive_style(configs: Option<&OzmuxConfigsResource>) -> PaneInactiveStyle {
    match configs {
        Some(cfg) if cfg.inactive_pane.enabled => {
            let (r, g, b) = cfg.inactive_pane.tint_color_rgb();
            let lin = Color::srgb_u8(r, g, b).to_linear();
            PaneInactiveStyle {
                dim: cfg.inactive_pane.dim,
                tint: Vec4::new(lin.red, lin.green, lin.blue, cfg.inactive_pane.tint),
                overlay_dim: cfg.inactive_pane.webview_dim,
                overlay_desaturate: cfg.inactive_pane.webview_desaturate,
            }
        }
        _ => PaneInactiveStyle::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ozmux_tmux::TmuxPane;

    use tmux_control_parser::{CellDims, PaneId};

    fn dims() -> CellDims {
        CellDims {
            width: 10,
            height: 5,
            xoff: 0,
            yoff: 0,
        }
    }

    #[test]
    fn active_pane_gains_keyboard_focus_and_inactive_loses_it() {
        use ozmux_tmux::{ActivePane, PaneId, TmuxPane};
        use tmux_control_parser::CellDims;

        let dims = CellDims {
            width: 80,
            height: 24,
            xoff: 0,
            yoff: 0,
        };
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_systems(
            Update,
            super::sync_keyboard_focus_to_active_pane.run_if(super::pane_active_state_changed),
        );

        let p1 = app
            .world_mut()
            .spawn((
                TmuxPane {
                    id: PaneId(1),
                    dims,
                },
                ActivePane,
            ))
            .id();
        let p2 = app
            .world_mut()
            .spawn(TmuxPane {
                id: PaneId(2),
                dims,
            })
            .id();
        app.update();
        assert!(
            app.world().entity(p1).contains::<KeyboardFocused>(),
            "active pane gets focus"
        );
        assert!(
            !app.world().entity(p2).contains::<KeyboardFocused>(),
            "inactive pane has none"
        );

        app.world_mut().entity_mut(p1).remove::<ActivePane>();
        app.world_mut().entity_mut(p2).insert(ActivePane);
        app.update();
        assert!(
            !app.world().entity(p1).contains::<KeyboardFocused>(),
            "former active loses focus"
        );
        assert!(
            app.world().entity(p2).contains::<KeyboardFocused>(),
            "new active gains focus"
        );
    }

    #[test]
    fn keyboard_focus_is_fresh_in_focusedkey_after_active_change() {
        use crate::surface::OzmaTerminal;
        use ozmux_tmux::ActivePane;

        #[derive(Resource, Default)]
        struct ProbeFocus(Option<Entity>);

        fn probe(mut seen: ResMut<ProbeFocus>, focused: Query<Entity, With<KeyboardFocused>>) {
            seen.0 = focused.single().ok();
        }

        let mut app = App::new();
        app.add_plugins((MinimalPlugins, PaneFocusPlugin))
            .init_resource::<ProbeFocus>()
            .add_systems(Update, probe.in_set(InputPhase::FocusedKey));

        let p1 = app
            .world_mut()
            .spawn((
                OzmaTerminal,
                TmuxPane {
                    id: PaneId(1),
                    dims: dims(),
                },
                ActivePane,
            ))
            .id();
        let p2 = app
            .world_mut()
            .spawn((
                OzmaTerminal,
                TmuxPane {
                    id: PaneId(2),
                    dims: dims(),
                },
            ))
            .id();
        app.update();

        // ActivePane moves p1 -> p2 this tick. PaneFocusPlugin's
        // .before(InputPhase::FocusedKey) edge must flush the deferred
        // KeyboardFocused move before a FocusedKey system reads it.
        app.world_mut().entity_mut(p1).remove::<ActivePane>();
        app.world_mut().entity_mut(p2).insert(ActivePane);
        app.update();

        assert_eq!(
            app.world().resource::<ProbeFocus>().0,
            Some(p2),
            "a system in InputPhase::FocusedKey sees KeyboardFocused already moved to the new \
             ActivePane the same frame — the real PaneFocusPlugin .before(FocusedKey) edge makes \
             it fresh, not a frame stale on the former active pane"
        );
    }

    #[test]
    fn sync_sets_pane_dim_from_active_marker() {
        use ozmux_tmux::ActivePane;

        let mut app = App::new();
        app.add_plugins((MinimalPlugins, PaneFocusPlugin));
        app.insert_resource(OzmuxConfigsResource::default());
        let h = || TerminalHandle::detached(10, 5);

        let p1 = app
            .world_mut()
            .spawn((
                TmuxPane {
                    id: PaneId(1),
                    dims: dims(),
                },
                h(),
                ActivePane,
            ))
            .id();

        let p2 = app
            .world_mut()
            .spawn((
                TmuxPane {
                    id: PaneId(2),
                    dims: dims(),
                },
                h(),
            ))
            .id();

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
            Some(PaneInactiveStyle::default()),
            "active pane: untinted, full-bright"
        );
        assert_eq!(
            style(&app, p2),
            Some(inactive),
            "inactive pane: background-tinted"
        );

        // Move ActivePane to p2.
        app.world_mut().entity_mut(p1).remove::<ActivePane>();
        app.world_mut().entity_mut(p2).insert(ActivePane);
        app.update();
        assert_eq!(style(&app, p1), Some(inactive));
        assert_eq!(style(&app, p2), Some(PaneInactiveStyle::default()));

        // No active pane: both untinted.
        app.world_mut().entity_mut(p2).remove::<ActivePane>();
        app.update();
        assert_eq!(style(&app, p1), Some(PaneInactiveStyle::default()));
        assert_eq!(style(&app, p2), Some(PaneInactiveStyle::default()));
    }

    #[test]
    fn disabled_config_leaves_every_pane_untreated() {
        use ozmux_tmux::ActivePane;

        let mut app = App::new();
        app.add_plugins((MinimalPlugins, PaneFocusPlugin));
        let mut configs = OzmuxConfigsResource::default();
        configs.0.inactive_pane.enabled = false;
        app.insert_resource(configs);
        let h = || TerminalHandle::detached(10, 5);

        let p1 = app
            .world_mut()
            .spawn((
                TmuxPane {
                    id: PaneId(1),
                    dims: dims(),
                },
                h(),
                ActivePane,
            ))
            .id();
        let p2 = app
            .world_mut()
            .spawn((
                TmuxPane {
                    id: PaneId(2),
                    dims: dims(),
                },
                h(),
            ))
            .id();

        app.update();
        let style = |app: &App, e| app.world().get::<PaneInactiveStyle>(e).copied();
        assert_eq!(style(&app, p1), Some(PaneInactiveStyle::default()));
        assert_eq!(
            style(&app, p2),
            Some(PaneInactiveStyle::default()),
            "enabled=false: inactive pane is untreated"
        );
    }

    #[test]
    fn augment_adds_button_and_focus_block_no_overlay() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, PaneFocusPlugin));
        let pane = app
            .world_mut()
            .spawn((
                TmuxPane {
                    id: PaneId(1),
                    dims: dims(),
                },
                TerminalHandle::detached(10, 5),
            ))
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
}
