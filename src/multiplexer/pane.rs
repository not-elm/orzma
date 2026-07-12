//! Multiplexer pane domain: the pane component, its cwd cache, and the
//! split-pane request observer.

pub(crate) mod exit;
pub(crate) mod layout;
pub(crate) mod spawn;

use crate::input::focus::KeyboardFocused;
use crate::multiplexer::bootstrap::{OrzmaTerminalConfig, WindowContainer};
use crate::multiplexer::layout::{PaneRect, SplitAxis};
use crate::multiplexer::pane::layout::PANE_GAP_PX;
use crate::multiplexer::pane::spawn::{MultiplexerPaneBundle, MultiplexerPaneSpawnOptions};
use crate::multiplexer::request::SplitPaneRequest;
use crate::multiplexer::window::{MultiplexerLayoutComp, MultiplexerWindow};
use bevy::prelude::*;
use bevy::ui::ComputedNode;
use orzma_tty_renderer::TerminalCellMetricsResource;
use orzma_webview::ControlPlaneHandle;
use std::path::PathBuf;

/// A multiplexer pane: a terminal surface owned by a window.
#[derive(Component)]
pub(crate) struct MultiplexerPane {
    /// The window this pane belongs to.
    pub window: Entity,
}

/// The pane's last OSC-7 reported cwd, used to seed a sibling's cwd on split.
#[derive(Component, Default)]
pub(crate) struct PaneCwd(pub Option<PathBuf>);

/// Registers the split-pane request observer.
pub(in crate::multiplexer) struct SplitPanePlugin;

impl Plugin for SplitPanePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_split_pane);
    }
}

/// Minimum pane extent, in cells, along a split's axis. Below this a
/// resulting child pane would be illegibly small.
const MIN_PANE_CELLS: f32 = 2.0;

/// Observer: consumes `SplitPaneRequest`, splitting the target pane's leaf
/// into two siblings along `axis` (spec §Data flow Split).
///
/// Un-zooms the window first, then rejects (no-op + warn, no PTY spawned) a
/// split that would shrink either resulting child below `MIN_PANE_CELLS`
/// cells. Otherwise spawns the new pane's PTY — shell from
/// `OrzmaTerminalConfig` (the same override `ensure_bootstrap` honors), cwd
/// seeded from the target's cached `PaneCwd`, env from `ControlPlaneHandle`
/// — BEFORE mutating the tree, so a failed spawn leaves the layout
/// untouched. On success, inserts the new leaf, moves the window's
/// `active_pane`, and parents the new pane
/// under its OWN fresh pane container (never the target's), so the exit
/// cascade's per-pane `ChildOf`-despawn (`pane/exit.rs`) never takes a
/// surviving sibling down with it.
fn on_split_pane(
    ev: On<SplitPaneRequest>,
    mut commands: Commands,
    mut windows: Query<&mut MultiplexerWindow>,
    mut layouts: Query<&mut MultiplexerLayoutComp>,
    config: Res<OrzmaTerminalConfig>,
    panes: Query<(&MultiplexerPane, &PaneCwd)>,
    containers: Query<(Entity, &WindowContainer, &ComputedNode)>,
    metrics: Option<Res<TerminalCellMetricsResource>>,
    control: Option<Res<ControlPlaneHandle>>,
) {
    let target = ev.event_target();
    let axis = ev.axis;
    let Ok((pane, cwd)) = panes.get(target) else {
        return;
    };
    let window = pane.window;
    let Ok(mut layout) = layouts.get_mut(window) else {
        return;
    };
    let Some((container, _, computed)) = containers.iter().find(|(_, c, _)| c.window == window)
    else {
        return;
    };
    let Some(metrics) = metrics else {
        tracing::warn!("split requested before terminal cell metrics are available; ignoring");
        return;
    };

    if layout.0.zoomed().is_some() {
        layout.0.set_zoom(None);
    }

    let area = PaneRect {
        x: 0.0,
        y: 0.0,
        w: computed.size.x,
        h: computed.size.y,
    };
    let cell_w = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h = metrics.metrics.line_height_phys.floor().max(1.0);
    let Some((_, rect)) = layout
        .0
        .rects(area, PANE_GAP_PX)
        .into_iter()
        .find(|(e, _)| *e == target)
    else {
        tracing::warn!(?target, "split target is not a leaf of its window's layout");
        return;
    };
    let (extent, cell_px) = match axis {
        SplitAxis::Vertical => (rect.w, cell_w),
        SplitAxis::Horizontal => (rect.h, cell_h),
    };
    if split_would_underflow(extent, cell_px, PANE_GAP_PX) {
        tracing::warn!(
            ?target,
            ?axis,
            "split would shrink a pane below the minimum size; ignoring"
        );
        return;
    }

    let new_pane = commands.spawn_empty().id();
    let env = control
        .as_deref()
        .map(|c| c.surface_env(new_pane).to_vec())
        .unwrap_or_default();
    match MultiplexerPaneBundle::spawn(MultiplexerPaneSpawnOptions {
        shell: config.shell.clone(),
        cwd: cwd.0.clone(),
        env,
    }) {
        Ok(bundle) => {
            layout.0.split(target, new_pane, axis);
            if let Ok(mut w) = windows.get_mut(window) {
                w.active_pane = new_pane;
            }
            let pane_container = commands
                .spawn((
                    Name::new("Pane Container"),
                    Node {
                        width: Val::Percent(100.0),
                        height: Val::Percent(100.0),
                        ..default()
                    },
                    ChildOf(container),
                ))
                .id();
            commands.entity(new_pane).insert((
                bundle,
                KeyboardFocused,
                MultiplexerPane { window },
                ChildOf(pane_container),
            ));
            // NOTE: bind the token only after a successful spawn, mirroring
            // ensure_bootstrap — a pre-spawn bind would leak the token if
            // the PTY spawn had failed instead.
            if let Some(c) = control.as_deref() {
                c.bind_surface(new_pane);
            }
        }
        Err(e) => {
            commands.entity(new_pane).despawn();
            tracing::warn!(?e, ?target, ?axis, "failed to spawn split pane");
        }
    }
}

/// Whether splitting a pane whose extent (px) along the split axis is
/// `extent_px` would shrink either resulting child below `MIN_PANE_CELLS`
/// cells.
///
/// `MultiplexerLayout::split` always starts a new split at ratio 0.5, and
/// `split_rect` subtracts one `gap_px` gutter before dividing the remainder
/// evenly between the two children, so each child gets roughly
/// `(extent_px - gap_px) / 2`. Requiring that to be at least
/// `MIN_PANE_CELLS * cell_px` per child is equivalent to requiring
/// `extent_px >= 2.0 * MIN_PANE_CELLS * cell_px + gap_px`.
fn split_would_underflow(extent_px: f32, cell_px: f32, gap_px: f32) -> bool {
    extent_px < 2.0 * MIN_PANE_CELLS * cell_px + gap_px
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multiplexer::layout::MultiplexerLayout;

    #[test]
    fn pane_component_roundtrips() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        let window = app.world_mut().spawn_empty().id();
        let pane = app
            .world_mut()
            .spawn((MultiplexerPane { window }, PaneCwd::default()))
            .id();
        let p = app.world().entity(pane).get::<MultiplexerPane>().unwrap();
        assert_eq!(p.window, window);
        let cwd = app.world().entity(pane).get::<PaneCwd>().unwrap();
        assert_eq!(cwd.0, None);
    }

    #[test]
    fn split_would_underflow_below_two_cells() {
        // cell_px = 8.0, gap_px = 1.0 -> threshold = 2.0 * 2.0 * 8.0 + 1.0 = 33.0
        assert!(split_would_underflow(32.0, 8.0, 1.0));
    }

    #[test]
    fn split_would_underflow_at_or_above_threshold_is_false() {
        assert!(!split_would_underflow(33.0, 8.0, 1.0));
        assert!(!split_would_underflow(64.0, 8.0, 1.0));
    }

    fn cell_metrics() -> TerminalCellMetricsResource {
        use orzma_tty_renderer::CellMetrics;
        TerminalCellMetricsResource {
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
        }
    }

    /// Spawns a one-pane window: a window entity, its `WindowContainer` (with
    /// a `ComputedNode` of `area_size`), a dedicated pane container, and the
    /// pane itself (with `PaneCwd::default()`), wired exactly like
    /// `ensure_bootstrap` and `on_split_pane` wire a real pane. Returns
    /// `(window, window_container, pane)`.
    fn spawn_one_pane_window(app: &mut App, area_size: Vec2) -> (Entity, Entity, Entity) {
        let world = app.world_mut();
        let window = world.spawn_empty().id();
        let window_container = world
            .spawn((
                WindowContainer { window },
                ComputedNode {
                    size: area_size,
                    ..ComputedNode::DEFAULT
                },
            ))
            .id();
        let pane_container = world.spawn(ChildOf(window_container)).id();
        let pane = world
            .spawn((
                MultiplexerPane { window },
                PaneCwd::default(),
                ChildOf(pane_container),
            ))
            .id();
        world.entity_mut(window).insert((
            MultiplexerWindow {
                index: 0,
                name: None,
                active_pane: pane,
            },
            MultiplexerLayoutComp(MultiplexerLayout::new(pane)),
        ));
        (window, window_container, pane)
    }

    fn build_app(metrics: TerminalCellMetricsResource, shell: Option<String>) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(metrics);
        app.insert_resource(OrzmaTerminalConfig { shell });
        app.add_observer(on_split_pane);
        app
    }

    #[test]
    fn split_below_minimum_size_is_a_noop_and_spawns_no_pane() {
        let mut app = build_app(cell_metrics(), None);
        // area.w = 20.0 is below the vertical-split threshold of 33.0 for
        // this cell/gap combination, so the split must be rejected.
        let (window, _, pane) = spawn_one_pane_window(&mut app, Vec2::new(20.0, 600.0));

        app.world_mut().trigger(SplitPaneRequest {
            pane,
            axis: SplitAxis::Vertical,
        });
        app.world_mut().flush();
        app.update();

        let world = app.world_mut();
        let mut panes = world.query_filtered::<(), With<MultiplexerPane>>();
        assert_eq!(
            panes.iter(world).count(),
            1,
            "a too-small split must not spawn a new pane"
        );
        assert_eq!(
            world
                .get::<MultiplexerLayoutComp>(window)
                .unwrap()
                .0
                .leaves()
                .len(),
            1,
            "the layout tree must be unchanged"
        );
    }

    #[test]
    fn split_success_spawns_sibling_and_moves_active_pane() {
        let mut app = build_app(cell_metrics(), None);
        let (window, window_container, target) =
            spawn_one_pane_window(&mut app, Vec2::new(800.0, 600.0));
        let target_parent = app
            .world()
            .entity(target)
            .get::<ChildOf>()
            .unwrap()
            .parent();

        app.world_mut().trigger(SplitPaneRequest {
            pane: target,
            axis: SplitAxis::Vertical,
        });
        app.world_mut().flush();
        app.update();

        let world = app.world_mut();
        let mut panes = world.query_filtered::<Entity, With<MultiplexerPane>>();
        let all_panes: Vec<Entity> = panes.iter(world).collect();
        assert_eq!(all_panes.len(), 2, "split must spawn exactly one sibling");
        let new_pane = *all_panes.iter().find(|p| **p != target).unwrap();

        let layout = &world.get::<MultiplexerLayoutComp>(window).unwrap().0;
        assert_eq!(layout.leaves().len(), 2, "the tree must gain a leaf");
        assert!(layout.contains(target));
        assert!(layout.contains(new_pane));

        assert_eq!(
            world.get::<MultiplexerWindow>(window).unwrap().active_pane,
            new_pane,
            "active_pane must move to the new sibling"
        );
        assert!(
            world.entity(new_pane).contains::<KeyboardFocused>(),
            "the new pane must be inserted with KeyboardFocused"
        );

        let new_parent = world.entity(new_pane).get::<ChildOf>().unwrap().parent();
        assert_ne!(
            new_parent, target_parent,
            "the new pane must get its own pane container, not share the target's"
        );
        let new_parents_parent = world.entity(new_parent).get::<ChildOf>().unwrap().parent();
        assert_eq!(
            new_parents_parent, window_container,
            "the new pane container must be parented under the window's WindowContainer"
        );
    }

    #[test]
    fn split_with_unspawnable_shell_leaves_tree_unchanged() {
        // NOTE: on macOS, `spawn_login_shell` wraps the shell in `/usr/bin/login`
        // (an executable that exists), so a merely-nonexistent path fails only
        // inside the grandchild `zsh -fc "exec ..."`, not synchronously — a plain
        // bad path here would make this test flake between "spawned" and "not
        // spawned" depending on platform. A NUL byte breaks the `CString`
        // conversion `std::process::Command::spawn` performs on every arg
        // (including the embedded exec string) before it forks, so the PTY spawn
        // fails synchronously and portably.
        let mut app = build_app(
            cell_metrics(),
            Some("/nonexistent/shell\0-does-not-exist".into()),
        );
        let (window, _, target) = spawn_one_pane_window(&mut app, Vec2::new(800.0, 600.0));

        app.world_mut().trigger(SplitPaneRequest {
            pane: target,
            axis: SplitAxis::Vertical,
        });
        app.world_mut().flush();
        app.update();

        let world = app.world_mut();
        let mut panes = world.query_filtered::<(), With<MultiplexerPane>>();
        assert_eq!(
            panes.iter(world).count(),
            1,
            "a failed PTY spawn must not leave a new pane behind"
        );
        assert_eq!(
            world
                .get::<MultiplexerLayoutComp>(window)
                .unwrap()
                .0
                .leaves()
                .len(),
            1,
            "the layout tree must be unchanged when the spawn fails"
        );
        assert_eq!(
            world.get::<MultiplexerWindow>(window).unwrap().active_pane,
            target,
            "active_pane must be unchanged when the spawn fails"
        );
    }
}
