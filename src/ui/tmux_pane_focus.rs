//! Pane augmentation and dim: gives each tmux pane a `Button` click target
//! with `FocusPolicy::Block` (load-bearing: stops pane clicks reaching webview
//! surfaces), and dims and desaturates every inactive pane at the renderer via
//! `PaneInactiveStyle` (brightness + grey-out the terminal shader applies)
//! rather than an opaque overlay veil. `select-pane` on press is now owned by
//! the tmux mouse gesture arbiter (`tmux_mouse::OzmuxTmuxMousePlugin`).

use crate::configs::OzmuxConfigsResource;
use bevy::prelude::*;
use bevy::ui::FocusPolicy;
use ozma_tty_engine::TerminalHandle;
use ozma_tty_renderer::material::PaneInactiveStyle;
use ozmux_tmux::{ActivePane, TmuxPane, TmuxProjectionSet};

/// Registers the pane augmentation (adds `Button` + `FocusPolicy::Block`) and
/// dim systems. `select-pane` on press is handled by the gesture arbiter.
pub struct OzmuxTmuxPaneFocusPlugin;

impl Plugin for OzmuxTmuxPaneFocusPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                augment_tmux_pane.after(TmuxProjectionSet),
                sync_inactive_pane_style.run_if(pane_active_state_changed),
            ),
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
/// active) gets the default no-op `{ dim: 1.0, desaturate: 0.0 }`; inactive
/// panes get the configured `(dim, desaturate)`. Only inserts when the value
/// changes. Gated by `pane_active_state_changed`, so it does not observe live
/// config edits — a config reload re-applies on the next active-pane change.
fn sync_inactive_pane_style(
    mut commands: Commands,
    panes: Query<(Entity, Has<ActivePane>, Option<&PaneInactiveStyle>), With<TmuxPane>>,
    configs: Option<Res<OzmuxConfigsResource>>,
) {
    let (dim, desaturate) = inactive_style(configs.as_deref());
    let any_active = panes.iter().any(|(_, active, _)| active);
    for (entity, active, current) in panes.iter() {
        let want = if active || !any_active {
            PaneInactiveStyle::default()
        } else {
            PaneInactiveStyle { dim, desaturate }
        };
        if current.copied() != Some(want) {
            commands.entity(entity).insert(want);
        }
    }
}

/// Returns the `(dim, desaturate)` applied to inactive panes: the configured
/// values when the inactive-pane treatment is enabled, otherwise the no-op
/// `(1.0, 0.0)`.
fn inactive_style(configs: Option<&OzmuxConfigsResource>) -> (f32, f32) {
    match configs {
        Some(cfg) if cfg.inactive_pane.enabled => {
            (cfg.inactive_pane.dim, cfg.inactive_pane.desaturate)
        }
        _ => (1.0, 0.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ozmux_tmux::TmuxPane;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
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
    fn sync_sets_pane_dim_from_active_marker() {
        use ozmux_tmux::ActivePane;

        let mut app = App::new();
        app.add_plugins((MinimalPlugins, OzmuxTmuxPaneFocusPlugin));
        app.insert_non_send_resource(ozmux_tmux::TmuxConnection::default());
        app.insert_resource(OzmuxConfigsResource::default());
        let h = || TerminalHandle::detached(10, 5, Arc::new(AtomicBool::new(false)));

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

        app.update();
        assert_eq!(
            style(&app, p1),
            Some(PaneInactiveStyle::default()),
            "active pane: full-bright, full-color"
        );
        assert_eq!(
            style(&app, p2),
            Some(PaneInactiveStyle {
                dim: 0.5,
                desaturate: 0.7
            }),
            "inactive pane: dimmed + desaturated"
        );

        // Move ActivePane to p2.
        app.world_mut().entity_mut(p1).remove::<ActivePane>();
        app.world_mut().entity_mut(p2).insert(ActivePane);
        app.update();
        assert_eq!(
            style(&app, p1),
            Some(PaneInactiveStyle {
                dim: 0.5,
                desaturate: 0.7
            })
        );
        assert_eq!(style(&app, p2), Some(PaneInactiveStyle::default()));

        // No active pane: both full-bright, full-color.
        app.world_mut().entity_mut(p2).remove::<ActivePane>();
        app.update();
        assert_eq!(style(&app, p1), Some(PaneInactiveStyle::default()));
        assert_eq!(style(&app, p2), Some(PaneInactiveStyle::default()));
    }

    #[test]
    fn disabled_config_leaves_every_pane_untreated() {
        use ozmux_tmux::ActivePane;

        let mut app = App::new();
        app.add_plugins((MinimalPlugins, OzmuxTmuxPaneFocusPlugin));
        app.insert_non_send_resource(ozmux_tmux::TmuxConnection::default());
        let mut configs = OzmuxConfigsResource::default();
        configs.0.inactive_pane.enabled = false;
        app.insert_resource(configs);
        let h = || TerminalHandle::detached(10, 5, Arc::new(AtomicBool::new(false)));

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
        app.add_plugins((MinimalPlugins, OzmuxTmuxPaneFocusPlugin));
        app.insert_non_send_resource(ozmux_tmux::TmuxConnection::default());
        let pane = app
            .world_mut()
            .spawn((
                TmuxPane {
                    id: PaneId(1),
                    dims: dims(),
                },
                TerminalHandle::detached(10, 5, Arc::new(AtomicBool::new(false))),
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
