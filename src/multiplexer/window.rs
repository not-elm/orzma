//! Multiplexer window (tab) domain: the window component, the active-window
//! marker, and the ECS wrapper around the pure layout tree.

use crate::input::InputPhase;
use crate::input::focus::KeyboardFocused;
use crate::multiplexer::bootstrap::{OrzmaTerminalConfig, WindowContainer};
use crate::multiplexer::layout::{MultiplexerLayout, PaneRect};
use crate::multiplexer::pane::layout::PANE_GAP_PX;
use crate::multiplexer::pane::spawn::{MultiplexerPaneBundle, MultiplexerPaneSpawnOptions};
use crate::multiplexer::pane::{MultiplexerPane, PaneCwd};
use crate::multiplexer::request::{
    KillWindowRequest, NewWindowRequest, SelectPaneRequest, SelectWindowRequest, WindowSelect,
};
use crate::ui::multiplexer::WorkspaceContainer;
use bevy::prelude::*;
use bevy::ui::ComputedNode;
use orzma_webview::ControlPlaneHandle;
use std::path::PathBuf;

/// A multiplexer window (tab). One is active at a time (see `ActiveMultiplexerWindow`).
#[derive(Component)]
pub(crate) struct MultiplexerWindow {
    /// Window-bar order and `select_window_N` target.
    pub index: u32,
    /// User-assigned name; `None` displays the active pane's `TerminalTitle`.
    pub name: Option<String>,
    /// The focused pane in this window, restored on switch.
    pub active_pane: Entity,
}

/// Marks the single active window whose `active_pane` drives keyboard focus.
#[derive(Component)]
pub(crate) struct ActiveMultiplexerWindow;

/// ECS wrapper around the Bevy-free layout tree, kept a newtype so
/// `layout.rs` has no Bevy dependency beyond the `Entity` id.
#[derive(Component)]
pub(crate) struct MultiplexerLayoutComp(pub MultiplexerLayout);

/// Registers the `SelectPaneRequest`/`NewWindowRequest`/`SelectWindowRequest`
/// messages, the `KillWindowRequest` observer, the
/// `On<Add, ActiveMultiplexerWindow>` visibility observer, `select_pane`,
/// `select_window`, `on_new_window`, `on_kill_window`, and
/// `sync_keyboard_focus_to_active_pane`.
///
/// Registers `SelectPaneRequest`/`NewWindowRequest`/`SelectWindowRequest`
/// here (not only in the shortcut-applier plugin that writes them) so
/// `select_pane`'s `on_message::<SelectPaneRequest>`, `on_new_window`'s
/// `on_message::<NewWindowRequest>`, and `select_window`'s
/// `on_message::<SelectWindowRequest>` run conditions have a `Messages<T>`
/// resource to read even when `WindowPlugin` is exercised without the input
/// plugins (as bootstrap tests do); `add_message` is idempotent, so this is
/// a no-op when the shortcut-applier plugin already registered them.
pub(in crate::multiplexer) struct WindowPlugin;

impl Plugin for WindowPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<SelectPaneRequest>()
            .add_message::<NewWindowRequest>()
            .add_message::<SelectWindowRequest>()
            .add_systems(
                Update,
                (
                    select_pane
                        .before(sync_keyboard_focus_to_active_pane)
                        .run_if(on_message::<SelectPaneRequest>),
                    sync_keyboard_focus_to_active_pane
                        .before(InputPhase::FocusedKey)
                        .run_if(active_pane_changed),
                    on_new_window.run_if(on_message::<NewWindowRequest>),
                    select_window.run_if(on_message::<SelectWindowRequest>),
                ),
            )
            .add_observer(on_kill_window)
            .add_observer(on_active_window_added);
    }
}

/// Moves `KeyboardFocused` onto the active window's `active_pane`: removes it
/// from whatever pane currently holds it, then inserts it on the new target.
/// Gated by `active_pane_changed` so it only writes on a real change; ordered
/// `.before(InputPhase::FocusedKey)` so the resolved key effects' `focused`
/// reflects the active pane the same frame it changes.
///
/// `pub(crate)` (not private): `crate::input::mouse::button::multiplexer`'s
/// `focus_pane_on_click` names this function in a `.before(...)` ordering
/// constraint, so its own `active_pane` write lands before this sync runs —
/// otherwise the two systems' conflicting `MultiplexerWindow` access would be
/// ordered arbitrarily and focus could lag a frame behind a click.
pub(crate) fn sync_keyboard_focus_to_active_pane(
    mut commands: Commands,
    active_windows: Query<&MultiplexerWindow, With<ActiveMultiplexerWindow>>,
    focused: Query<Entity, With<KeyboardFocused>>,
) {
    let Ok(window) = active_windows.single() else {
        return;
    };
    let target = window.active_pane;
    for current in focused.iter() {
        if current != target {
            commands.entity(current).remove::<KeyboardFocused>();
        }
    }
    if !focused.contains(target) {
        commands.entity(target).insert(KeyboardFocused);
    }
}

/// Moves the active window's `active_pane` to its neighbor in a
/// `SelectPaneRequest`'s direction. Un-zooms the window first so the
/// neighbor lookup runs against the full split layout rather than the
/// zoomed pane's full-area rect. `active_pane` is written only when the
/// computed neighbor differs from the current one, so a request at the
/// window edge (no neighbor in `dir`) is a no-op that never spuriously
/// trips `Changed<MultiplexerWindow>`.
///
/// Ordered `.before(sync_keyboard_focus_to_active_pane)` so a focus change
/// lands the same frame `active_pane` moves; gated
/// `.run_if(on_message::<SelectPaneRequest>)`.
fn select_pane(
    mut requests: MessageReader<SelectPaneRequest>,
    mut windows: Query<
        (Entity, &mut MultiplexerWindow, &mut MultiplexerLayoutComp),
        With<ActiveMultiplexerWindow>,
    >,
    containers: Query<(&WindowContainer, &ComputedNode)>,
) {
    let Ok((window, mut window_state, mut layout)) = windows.single_mut() else {
        return;
    };
    let Some((_, computed)) = containers.iter().find(|(c, _)| c.window == window) else {
        return;
    };
    let area = PaneRect {
        x: 0.0,
        y: 0.0,
        w: computed.size.x,
        h: computed.size.y,
    };
    for msg in requests.read() {
        if layout.0.zoomed().is_some() {
            layout.0.set_zoom(None);
        }
        if let Some(next) = layout
            .0
            .neighbor(window_state.active_pane, msg.dir, area, PANE_GAP_PX)
            && next != window_state.active_pane
        {
            window_state.active_pane = next;
        }
    }
}

/// `MessageReader<SelectWindowRequest>` consumer: resolves the target window
/// via the pure `select_target` from the current active window's index, then
/// — only if the target exists and differs from the current active window —
/// moves `ActiveMultiplexerWindow` and `KeyboardFocused` onto it. A request
/// that resolves to `None` or to the already-active window is a no-op: the
/// marker and focus components are left untouched, so `On<Add,
/// ActiveMultiplexerWindow>` (`on_active_window_added`) never re-fires
/// spuriously.
///
/// Multiple requests in the same frame chain against an in-loop `active`/
/// `active_index` pair rather than re-reading the queries, mirroring
/// `on_new_window`: the `Commands` this system queues have not applied yet,
/// so the queries would otherwise still see the pre-switch state.
///
/// Gated `.run_if(on_message::<SelectWindowRequest>)`.
fn select_window(
    mut commands: Commands,
    mut requests: MessageReader<SelectWindowRequest>,
    active_windows: Query<(Entity, &MultiplexerWindow), With<ActiveMultiplexerWindow>>,
    all_windows: Query<(Entity, &MultiplexerWindow)>,
) {
    let Ok((mut active, active_state)) = active_windows.single() else {
        return;
    };
    let mut active_index = active_state.index;
    let windows: Vec<(Entity, u32)> = all_windows.iter().map(|(e, w)| (e, w.index)).collect();

    for msg in requests.read() {
        let Some(target) = select_target(&windows, active_index, msg.0) else {
            continue;
        };
        if target == active {
            continue;
        }
        let Ok((_, old_state)) = all_windows.get(active) else {
            continue;
        };
        let Ok((_, target_state)) = all_windows.get(target) else {
            continue;
        };
        commands.entity(active).remove::<ActiveMultiplexerWindow>();
        commands
            .entity(old_state.active_pane)
            .remove::<KeyboardFocused>();
        commands.entity(target).insert(ActiveMultiplexerWindow);
        commands
            .entity(target_state.active_pane)
            .insert(KeyboardFocused);
        active_index = target_state.index;
        active = target;
    }
}

/// Pure resolution of a `WindowSelect` against the current windows and the
/// active window's index: `Next`/`Previous` step to the neighboring index,
/// WRAPPING to the opposite end when there is no neighbor in that direction;
/// `Index(n)` resolves to the window whose index is exactly `n`, or `None` if
/// no window has it. With a single window, `Next`/`Previous` both resolve
/// back to that same window (the caller's no-op check then skips the move).
fn select_target(
    windows: &[(Entity, u32)],
    active_index: u32,
    sel: WindowSelect,
) -> Option<Entity> {
    match sel {
        WindowSelect::Next => windows
            .iter()
            .filter(|(_, index)| *index > active_index)
            .min_by_key(|(_, index)| *index)
            .or_else(|| windows.iter().min_by_key(|(_, index)| *index))
            .map(|(entity, _)| *entity),
        WindowSelect::Previous => windows
            .iter()
            .filter(|(_, index)| *index < active_index)
            .max_by_key(|(_, index)| *index)
            .or_else(|| windows.iter().max_by_key(|(_, index)| *index))
            .map(|(entity, _)| *entity),
        WindowSelect::Index(n) => windows
            .iter()
            .find(|(_, index)| *index == n as u32)
            .map(|(entity, _)| *entity),
    }
}

/// Whether the active window's `MultiplexerWindow` (and so, approximately,
/// its `active_pane`) changed this frame.
fn active_pane_changed(
    windows: Query<(), (With<ActiveMultiplexerWindow>, Changed<MultiplexerWindow>)>,
) -> bool {
    !windows.is_empty()
}

/// Reads the cwd to seed a new window's pane with: `active_pane`'s cached
/// `PaneCwd`, so a new window opens where the user was. `None` when the pane
/// has no cached cwd yet (or does not exist), falling back to inheriting the
/// spawning process's cwd.
fn seed_cwd(active_pane: Entity, panes: &Query<&PaneCwd>) -> Option<PathBuf> {
    panes.get(active_pane).ok().and_then(|cwd| cwd.0.clone())
}

/// Spawns a window+pane subtree under `workspace`, mirroring
/// `ensure_bootstrap`'s spawn shape (`crate::multiplexer::bootstrap`) for a
/// NEW (non-bootstrap) window: `WorkspaceContainer` -> `WindowContainer` ->
/// pane container -> pane, with the pane's PTY spawned from `config`/`cwd`/
/// `control`'s env, and — only on success — the pane bound on the control
/// plane. Returns `Some((window, pane))`, with `MultiplexerWindow`,
/// `ActiveMultiplexerWindow`, `MultiplexerLayoutComp` on `window` and the
/// pane bundle, `KeyboardFocused`, `MultiplexerPane` on `pane`.
///
/// On a failed PTY spawn, despawns every placeholder entity this call
/// created (unlike `ensure_bootstrap`'s error path, which keeps its
/// `WindowContainer` to satisfy a re-fire gate this call has none of) and
/// returns `None`; the caller treats that as a no-op.
fn spawn_window(
    commands: &mut Commands,
    workspace: Entity,
    index: u32,
    cwd: Option<PathBuf>,
    config: &OrzmaTerminalConfig,
    control: Option<&ControlPlaneHandle>,
) -> Option<(Entity, Entity)> {
    let window = commands.spawn_empty().id();
    let window_container = commands
        .spawn((
            Name::new("Window Container"),
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            WindowContainer { window },
            ChildOf(workspace),
        ))
        .id();
    let pane_container = commands
        .spawn((
            Name::new("Pane Container"),
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            ChildOf(window_container),
        ))
        .id();
    let pane = commands.spawn_empty().id();
    let env = control
        .map(|c| c.surface_env(pane).to_vec())
        .unwrap_or_default();
    match MultiplexerPaneBundle::spawn(MultiplexerPaneSpawnOptions {
        shell: config.shell.clone(),
        cwd,
        env,
    }) {
        Ok(bundle) => {
            commands.entity(pane).insert((
                bundle,
                KeyboardFocused,
                MultiplexerPane { window },
                ChildOf(pane_container),
            ));
            commands.entity(window).insert((
                MultiplexerWindow {
                    index,
                    name: None,
                    active_pane: pane,
                },
                ActiveMultiplexerWindow,
                MultiplexerLayoutComp(MultiplexerLayout::new(pane)),
            ));
            // NOTE: bind the token only after a successful spawn, mirroring
            // ensure_bootstrap — a pre-spawn bind would leak the token if the
            // PTY spawn had failed instead.
            if let Some(c) = control {
                c.bind_surface(pane);
            }
            Some((window, pane))
        }
        Err(e) => {
            commands.entity(pane).despawn();
            commands.entity(pane_container).despawn();
            commands.entity(window_container).despawn();
            commands.entity(window).despawn();
            tracing::error!(?e, "failed to spawn new multiplexer window");
            None
        }
    }
}

/// `MessageReader<NewWindowRequest>` consumer: spawns a new active window
/// per request via `spawn_window`, seeding the new pane's cwd from the
/// previously-active window's `active_pane`'s cached `PaneCwd`
/// (`seed_cwd`) and moving `ActiveMultiplexerWindow` + `KeyboardFocused`
/// from the old active window/pane onto the new ones. The new window's
/// `index` is one past the current max `MultiplexerWindow.index`.
///
/// A failed spawn (`spawn_window` returns `None`) is a no-op: the existing
/// active window/pane stay active, unlike `ensure_bootstrap`, whose failure
/// `AppExit`s because it is the only window.
///
/// Gated `.run_if(on_message::<NewWindowRequest>)`.
fn on_new_window(
    mut commands: Commands,
    mut requests: MessageReader<NewWindowRequest>,
    config: Res<OrzmaTerminalConfig>,
    control: Option<Res<ControlPlaneHandle>>,
    active_windows: Query<(Entity, &MultiplexerWindow), With<ActiveMultiplexerWindow>>,
    all_windows: Query<&MultiplexerWindow>,
    panes: Query<&PaneCwd>,
    workspace: Query<Entity, With<WorkspaceContainer>>,
) {
    let Ok(workspace) = workspace.single() else {
        return;
    };
    let Ok((mut active_window, active_state)) = active_windows.single() else {
        return;
    };
    let mut active_pane = active_state.active_pane;
    let mut next_index = all_windows
        .iter()
        .map(|w| w.index)
        .max()
        .map_or(0, |max| max + 1);

    for _ in requests.read() {
        let cwd = seed_cwd(active_pane, &panes);
        let Some((window, pane)) = spawn_window(
            &mut commands,
            workspace,
            next_index,
            cwd,
            &config,
            control.as_deref(),
        ) else {
            continue;
        };
        commands
            .entity(active_window)
            .remove::<ActiveMultiplexerWindow>();
        commands.entity(active_pane).remove::<KeyboardFocused>();
        active_window = window;
        active_pane = pane;
        next_index += 1;
    }
}

/// Picks the neighbor to activate when the active window at `killed_index`
/// closes: the remaining window with the largest index still less than
/// `killed_index` (the previous window), or — if there is none — the
/// smallest remaining index greater than it (the next window). `remaining`
/// need not be sorted.
fn pick_neighbor(remaining: &[(Entity, u32)], killed_index: u32) -> Option<Entity> {
    remaining
        .iter()
        .filter(|(_, index)| *index < killed_index)
        .max_by_key(|(_, index)| *index)
        .or_else(|| {
            remaining
                .iter()
                .filter(|(_, index)| *index > killed_index)
                .min_by_key(|(_, index)| *index)
        })
        .map(|(entity, _)| *entity)
}

/// Reassigns each of `entries`' `MultiplexerWindow.index` (looked up in
/// `windows`) to a contiguous `0..n`, in ascending order of the entry's
/// CURRENT index — closing whatever gap a removed window left. Writes
/// `index` only when the new value differs from the old one, so change
/// detection fires only for windows whose index actually moved (rust.md
/// change-detection rule).
fn renumber_windows(
    windows: &mut Query<(Entity, &mut MultiplexerWindow)>,
    entries: &[(Entity, u32)],
) {
    let mut ordered = entries.to_vec();
    ordered.sort_by_key(|(_, index)| *index);
    for (new_index, (entity, _)) in ordered.iter().enumerate() {
        let new_index = new_index as u32;
        let Ok((_, mut window)) = windows.get_mut(*entity) else {
            continue;
        };
        if window.index != new_index {
            window.index = new_index;
        }
    }
}

/// `On<KillWindowRequest>` observer: despawns the targeted window's subtree
/// — its `WindowContainer` (recursively taking its pane containers and
/// panes) plus the window entity itself — mirroring `close_pane`'s
/// last-leaf branch (`crate::multiplexer::pane::exit`) at window
/// granularity.
///
/// If the killed window was the only `MultiplexerWindow`, writes
/// `AppExit::Success` and returns. Otherwise reassigns the remaining
/// windows' `index` to close the gap (`renumber_windows`), and — if the
/// killed window was active — activates a neighbor (`pick_neighbor`) and
/// moves `KeyboardFocused` onto its `active_pane`.
///
/// The killed window's index, its active-ness, and every remaining window's
/// current index are snapshotted BEFORE any despawn command is queued:
/// `Commands` are deferred, so `windows` would otherwise still see the
/// killed entity right up to the next flush, and computing the neighbor
/// from a snapshot keeps that flush timing irrelevant to the result.
fn on_kill_window(
    ev: On<KillWindowRequest>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
    mut windows: Query<(Entity, &mut MultiplexerWindow)>,
    active: Query<Entity, With<ActiveMultiplexerWindow>>,
    containers: Query<(Entity, &WindowContainer)>,
) {
    let killed = ev.event_target();
    let Some(killed_index) = windows.get(killed).ok().map(|(_, window)| window.index) else {
        return;
    };
    let was_active = active.get(killed).is_ok();
    let last_window = windows.iter().count() <= 1;
    let remaining: Vec<(Entity, u32)> = windows
        .iter()
        .filter(|(entity, _)| *entity != killed)
        .map(|(entity, window)| (entity, window.index))
        .collect();

    if let Some((container, _)) = containers.iter().find(|(_, c)| c.window == killed) {
        commands.entity(container).despawn();
    }
    commands.entity(killed).despawn();

    if last_window {
        exit.write(AppExit::Success);
        return;
    }

    let neighbor = was_active
        .then(|| pick_neighbor(&remaining, killed_index))
        .flatten();
    renumber_windows(&mut windows, &remaining);

    if let Some(neighbor) = neighbor {
        commands.entity(neighbor).insert(ActiveMultiplexerWindow);
        if let Ok((_, window)) = windows.get(neighbor) {
            commands.entity(window.active_pane).insert(KeyboardFocused);
        }
    }
}

/// `On<Add, ActiveMultiplexerWindow>` observer: the ONLY place that writes a
/// `WindowContainer`'s `Node.display`. Fires whenever `ActiveMultiplexerWindow`
/// is added to a window — by `select_window` above, `on_new_window`, and
/// bootstrap alike — so this one hook keeps every container's visibility in
/// sync with whichever window just became active: `Display::Flex` for the
/// container whose `window` is the entity the marker was added to,
/// `Display::None` for every other container. Written only when the value
/// differs, so this never spuriously trips the `Changed<ComputedNode>` gate
/// that `apply_layout` (`pane/layout.rs`) and `reconcile_divider_handles`
/// (`ui/multiplexer/divider_handle.rs`) key off of — the layout reflow those
/// two systems drive is itself triggered by the `Display` flip changing
/// `ComputedNode.size` the next frame, so no manual recompute is needed here.
fn on_active_window_added(
    ev: On<Add, ActiveMultiplexerWindow>,
    mut containers: Query<(&WindowContainer, &mut Node)>,
) {
    let active = ev.event_target();
    for (container, mut node) in containers.iter_mut() {
        let want = if container.window == active {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != want {
            node.display = want;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multiplexer::layout::SplitAxis;
    use orzma_configs::shortcuts::PaneDirection;

    /// Spawns an active window with a 2-leaf vertical split (`pane_a` left,
    /// `pane_b` right, `active_pane` starting on `pane_a`), plus a
    /// `WindowContainer` carrying a `ComputedNode` of `area_size` so
    /// `select_pane` can resolve the workspace area. Returns
    /// `(window, pane_a, pane_b)`.
    fn spawn_two_pane_vertical_window(app: &mut App, area_size: Vec2) -> (Entity, Entity, Entity) {
        let world = app.world_mut();
        let pane_a = world.spawn_empty().id();
        let pane_b = world.spawn_empty().id();
        let mut layout = MultiplexerLayout::new(pane_a);
        layout.split(pane_a, pane_b, SplitAxis::Vertical);
        let window = world
            .spawn((
                MultiplexerWindow {
                    index: 0,
                    name: None,
                    active_pane: pane_a,
                },
                ActiveMultiplexerWindow,
                MultiplexerLayoutComp(layout),
            ))
            .id();
        world.spawn((
            WindowContainer { window },
            ComputedNode {
                size: area_size,
                ..ComputedNode::DEFAULT
            },
        ));
        (window, pane_a, pane_b)
    }

    #[test]
    fn select_pane_right_moves_active_pane_to_right_neighbor() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<SelectPaneRequest>()
            .add_systems(Update, select_pane);
        let (window, _pane_a, pane_b) =
            spawn_two_pane_vertical_window(&mut app, Vec2::new(800.0, 600.0));

        app.world_mut().write_message(SelectPaneRequest {
            dir: PaneDirection::Right,
        });
        app.update();

        assert_eq!(
            app.world()
                .get::<MultiplexerWindow>(window)
                .unwrap()
                .active_pane,
            pane_b,
            "SelectPaneRequest {{ Right }} from the left pane must move focus to its right neighbor"
        );
    }

    #[test]
    fn select_pane_left_moves_active_pane_to_left_neighbor() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<SelectPaneRequest>()
            .add_systems(Update, select_pane);
        let (window, pane_a, pane_b) =
            spawn_two_pane_vertical_window(&mut app, Vec2::new(800.0, 600.0));
        app.world_mut()
            .get_mut::<MultiplexerWindow>(window)
            .unwrap()
            .active_pane = pane_b;

        app.world_mut().write_message(SelectPaneRequest {
            dir: PaneDirection::Left,
        });
        app.update();

        assert_eq!(
            app.world()
                .get::<MultiplexerWindow>(window)
                .unwrap()
                .active_pane,
            pane_a,
            "SelectPaneRequest {{ Left }} from the right pane must move focus to its left neighbor"
        );
    }

    #[test]
    fn select_pane_with_no_neighbor_on_that_axis_is_noop() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<SelectPaneRequest>()
            .add_systems(Update, select_pane);
        let (window, pane_a, _pane_b) =
            spawn_two_pane_vertical_window(&mut app, Vec2::new(800.0, 600.0));

        app.world_mut().write_message(SelectPaneRequest {
            dir: PaneDirection::Up,
        });
        app.update();
        app.world_mut().write_message(SelectPaneRequest {
            dir: PaneDirection::Down,
        });
        app.update();

        assert_eq!(
            app.world()
                .get::<MultiplexerWindow>(window)
                .unwrap()
                .active_pane,
            pane_a,
            "a vertical split has no neighbor above/below; Up/Down must be a no-op"
        );
    }

    #[test]
    fn select_pane_unzooms_before_computing_neighbor() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<SelectPaneRequest>()
            .add_systems(Update, select_pane);
        let (window, pane_a, pane_b) =
            spawn_two_pane_vertical_window(&mut app, Vec2::new(800.0, 600.0));
        app.world_mut()
            .get_mut::<MultiplexerLayoutComp>(window)
            .unwrap()
            .0
            .set_zoom(Some(pane_a));

        app.world_mut().write_message(SelectPaneRequest {
            dir: PaneDirection::Right,
        });
        app.update();

        assert_eq!(
            app.world()
                .get::<MultiplexerLayoutComp>(window)
                .unwrap()
                .0
                .zoomed(),
            None,
            "select_pane must clear zoom before moving focus"
        );
        assert_eq!(
            app.world()
                .get::<MultiplexerWindow>(window)
                .unwrap()
                .active_pane,
            pane_b,
            "after un-zooming, Right must move focus to the right neighbor"
        );
    }

    #[test]
    fn select_pane_edge_request_does_not_trigger_change_detection() {
        #[derive(Resource, Default)]
        struct RunCount(u32);

        fn probe(mut count: ResMut<RunCount>) {
            count.0 += 1;
        }

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<SelectPaneRequest>()
            .init_resource::<RunCount>()
            .add_systems(
                Update,
                (select_pane, probe.run_if(active_pane_changed)).chain(),
            );
        let (window, pane_a, _pane_b) =
            spawn_two_pane_vertical_window(&mut app, Vec2::new(800.0, 600.0));

        app.update();
        assert_eq!(
            app.world().resource::<RunCount>().0,
            1,
            "a freshly-inserted MultiplexerWindow counts as changed"
        );

        app.world_mut().write_message(SelectPaneRequest {
            dir: PaneDirection::Up,
        });
        app.update();

        assert_eq!(
            app.world().resource::<RunCount>().0,
            1,
            "an edge SelectPaneRequest with no neighbor must not spuriously trip Changed<MultiplexerWindow>"
        );
        assert_eq!(
            app.world()
                .get::<MultiplexerWindow>(window)
                .unwrap()
                .active_pane,
            pane_a
        );
    }

    #[test]
    fn window_component_roundtrips() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        let pane = app.world_mut().spawn_empty().id();
        let win = app
            .world_mut()
            .spawn((
                MultiplexerWindow {
                    index: 0,
                    name: None,
                    active_pane: pane,
                },
                ActiveMultiplexerWindow,
            ))
            .id();
        let w = app.world().entity(win).get::<MultiplexerWindow>().unwrap();
        assert_eq!(w.index, 0);
        assert_eq!(w.active_pane, pane);
        assert!(
            app.world()
                .entity(win)
                .contains::<ActiveMultiplexerWindow>()
        );
    }

    #[test]
    fn active_pane_change_moves_keyboard_focus() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_systems(
            Update,
            sync_keyboard_focus_to_active_pane.run_if(active_pane_changed),
        );

        let pane_a = app.world_mut().spawn(KeyboardFocused).id();
        let pane_b = app.world_mut().spawn_empty().id();
        let window = app
            .world_mut()
            .spawn((
                MultiplexerWindow {
                    index: 0,
                    name: None,
                    active_pane: pane_a,
                },
                ActiveMultiplexerWindow,
            ))
            .id();

        app.update();
        assert!(
            app.world().entity(pane_a).contains::<KeyboardFocused>(),
            "the active window's active_pane already carries KeyboardFocused"
        );
        assert!(!app.world().entity(pane_b).contains::<KeyboardFocused>());

        app.world_mut()
            .entity_mut(window)
            .get_mut::<MultiplexerWindow>()
            .unwrap()
            .active_pane = pane_b;
        app.update();

        assert!(
            !app.world().entity(pane_a).contains::<KeyboardFocused>(),
            "the former active_pane loses focus"
        );
        assert!(
            app.world().entity(pane_b).contains::<KeyboardFocused>(),
            "the new active_pane gains focus"
        );
    }

    #[test]
    fn active_pane_changed_gates_on_real_change_only() {
        #[derive(Resource, Default)]
        struct RunCount(u32);

        fn probe(mut count: ResMut<RunCount>) {
            count.0 += 1;
        }

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<RunCount>();
        app.add_systems(Update, probe.run_if(active_pane_changed));

        let pane = app.world_mut().spawn_empty().id();
        let window = app
            .world_mut()
            .spawn((
                MultiplexerWindow {
                    index: 0,
                    name: None,
                    active_pane: pane,
                },
                ActiveMultiplexerWindow,
            ))
            .id();

        app.update();
        assert_eq!(
            app.world().resource::<RunCount>().0,
            1,
            "a freshly-inserted MultiplexerWindow counts as changed"
        );

        app.update();
        assert_eq!(
            app.world().resource::<RunCount>().0,
            1,
            "an untouched window must not re-trigger the gate"
        );

        app.world_mut()
            .entity_mut(window)
            .get_mut::<MultiplexerWindow>()
            .unwrap()
            .active_pane = pane;
        app.update();
        assert_eq!(
            app.world().resource::<RunCount>().0,
            2,
            "mutating the window must re-trigger the gate"
        );
    }

    #[test]
    fn seed_cwd_reads_active_panes_cached_cwd() {
        use bevy::ecs::system::SystemState;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        let seed_path = PathBuf::from("/tmp/seeded-cwd");
        let pane_with_cwd = app.world_mut().spawn(PaneCwd(Some(seed_path.clone()))).id();
        let pane_without_cwd = app.world_mut().spawn(PaneCwd::default()).id();

        let mut state: SystemState<Query<&PaneCwd>> = SystemState::new(app.world_mut());
        let panes = state
            .get(app.world())
            .expect("SystemState params must be valid for the test world");

        assert_eq!(
            seed_cwd(pane_with_cwd, &panes),
            Some(seed_path),
            "seed_cwd reads the active pane's cached PaneCwd"
        );
        assert_eq!(
            seed_cwd(pane_without_cwd, &panes),
            None,
            "a pane with no cached cwd seeds None"
        );
    }

    #[test]
    fn pick_neighbor_prefers_previous_window_then_next() {
        let mut world = World::new();
        let e0 = world.spawn_empty().id();
        let e2 = world.spawn_empty().id();
        let e5 = world.spawn_empty().id();
        let remaining = vec![(e0, 0), (e2, 2), (e5, 5)];

        assert_eq!(
            pick_neighbor(&remaining, 3),
            Some(e2),
            "prefers the largest remaining index below the killed one"
        );
        assert_eq!(
            pick_neighbor(&remaining, 0),
            Some(e2),
            "falls back to the smallest remaining index above when none is below"
        );
        assert_eq!(
            pick_neighbor(&[], 3),
            None,
            "no remaining windows means no neighbor to activate"
        );
    }

    #[test]
    fn renumber_windows_closes_gap() {
        fn run_renumber(mut windows: Query<(Entity, &mut MultiplexerWindow)>) {
            let entries: Vec<(Entity, u32)> = windows.iter().map(|(e, w)| (e, w.index)).collect();
            renumber_windows(&mut windows, &entries);
        }

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        let pane = app.world_mut().spawn_empty().id();
        let spawn_window_at = |app: &mut App, index: u32| {
            app.world_mut()
                .spawn(MultiplexerWindow {
                    index,
                    name: None,
                    active_pane: pane,
                })
                .id()
        };
        let w0 = spawn_window_at(&mut app, 0);
        let w2 = spawn_window_at(&mut app, 2);
        let w5 = spawn_window_at(&mut app, 5);
        app.add_systems(Update, run_renumber);

        app.update();

        assert_eq!(app.world().get::<MultiplexerWindow>(w0).unwrap().index, 0);
        assert_eq!(app.world().get::<MultiplexerWindow>(w2).unwrap().index, 1);
        assert_eq!(app.world().get::<MultiplexerWindow>(w5).unwrap().index, 2);
    }

    /// Spawns a window with a dummy `active_pane` entity (no PTY, no
    /// `WindowContainer`) — the minimal shape `on_kill_window`'s tests
    /// exercise. Returns `(window, pane)`.
    fn spawn_bare_window(app: &mut App, index: u32, active: bool) -> (Entity, Entity) {
        let world = app.world_mut();
        let pane = world.spawn_empty().id();
        let window = world
            .spawn(MultiplexerWindow {
                index,
                name: None,
                active_pane: pane,
            })
            .id();
        if active {
            world.entity_mut(window).insert(ActiveMultiplexerWindow);
        }
        (window, pane)
    }

    #[test]
    fn kill_window_renumbers_and_activates_neighbor() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<AppExit>();
        app.add_observer(on_kill_window);

        let (w0, p0) = spawn_bare_window(&mut app, 0, false);
        let (w1, _p1) = spawn_bare_window(&mut app, 1, true);
        let (w2, p2) = spawn_bare_window(&mut app, 2, false);

        app.world_mut().trigger(KillWindowRequest { window: w1 });
        app.world_mut().flush();
        app.update();

        assert!(
            app.world().get_entity(w1).is_err(),
            "the killed window must not survive"
        );
        assert_eq!(
            app.world().get::<MultiplexerWindow>(w0).unwrap().index,
            0,
            "the window before the gap keeps its index"
        );
        assert_eq!(
            app.world().get::<MultiplexerWindow>(w2).unwrap().index,
            1,
            "the window after the gap closes onto it"
        );
        assert!(
            app.world().entity(w0).contains::<ActiveMultiplexerWindow>(),
            "the previous window by index becomes active"
        );
        assert!(
            !app.world().entity(w2).contains::<ActiveMultiplexerWindow>(),
            "only the chosen neighbor becomes active"
        );
        assert!(
            app.world().entity(p0).contains::<KeyboardFocused>(),
            "keyboard focus moves to the new active window's active_pane"
        );
        assert!(!app.world().entity(p2).contains::<KeyboardFocused>());
    }

    #[test]
    fn kill_last_window_app_exits() {
        #[derive(Resource, Default)]
        struct Got(bool);
        fn capture(mut reader: MessageReader<AppExit>, mut got: ResMut<Got>) {
            if reader.read().next().is_some() {
                got.0 = true;
            }
        }
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<AppExit>();
        app.init_resource::<Got>();
        app.add_systems(Update, capture);
        app.add_observer(on_kill_window);

        let (window, _pane) = spawn_bare_window(&mut app, 0, true);

        app.world_mut().trigger(KillWindowRequest { window });
        app.world_mut().flush();
        app.update();

        assert!(
            app.world().resource::<Got>().0,
            "killing the last window must exit the app"
        );
        assert!(
            app.world().get_entity(window).is_err(),
            "the killed window must not survive"
        );
    }

    #[test]
    fn select_target_next_previous_wrap() {
        let mut world = World::new();
        let a = world.spawn_empty().id();
        let b = world.spawn_empty().id();
        let c = world.spawn_empty().id();
        let windows = [(a, 0), (b, 1), (c, 2)];

        assert_eq!(
            select_target(&windows, 2, WindowSelect::Next),
            Some(a),
            "Next from the highest index wraps to the smallest index"
        );
        assert_eq!(
            select_target(&windows, 0, WindowSelect::Previous),
            Some(c),
            "Previous from the smallest index wraps to the largest index"
        );
        assert_eq!(
            select_target(&windows, 0, WindowSelect::Next),
            Some(b),
            "Next steps to the next-higher index"
        );
        assert_eq!(
            select_target(&windows, 0, WindowSelect::Index(1)),
            Some(b),
            "Index resolves to the window with that exact index"
        );
        assert_eq!(
            select_target(&windows, 0, WindowSelect::Index(9)),
            None,
            "Index with no matching window resolves to None"
        );
    }

    #[test]
    fn select_window_moves_active_and_focus() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<SelectWindowRequest>()
            .add_systems(Update, select_window);

        let (window_a, pane_a) = spawn_bare_window(&mut app, 0, true);
        let (window_b, pane_b) = spawn_bare_window(&mut app, 1, false);
        app.world_mut().entity_mut(pane_a).insert(KeyboardFocused);

        app.world_mut()
            .write_message(SelectWindowRequest(WindowSelect::Next));
        app.update();

        assert!(
            app.world()
                .entity(window_b)
                .contains::<ActiveMultiplexerWindow>(),
            "Next from window 0 of two windows must activate window 1"
        );
        assert!(
            !app.world()
                .entity(window_a)
                .contains::<ActiveMultiplexerWindow>(),
            "the previously active window loses the marker"
        );
        assert!(
            app.world().entity(pane_b).contains::<KeyboardFocused>(),
            "keyboard focus follows to the new active window's active_pane"
        );
        assert!(
            !app.world().entity(pane_a).contains::<KeyboardFocused>(),
            "the old active pane loses KeyboardFocused"
        );
    }

    #[test]
    fn select_window_nonexistent_index_is_noop() {
        #[derive(Resource, Default)]
        struct AddCount(u32);
        fn count_added(_ev: On<Add, ActiveMultiplexerWindow>, mut count: ResMut<AddCount>) {
            count.0 += 1;
        }

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<SelectWindowRequest>()
            .init_resource::<AddCount>()
            .add_systems(Update, select_window)
            .add_observer(count_added);

        let (window_a, pane_a) = spawn_bare_window(&mut app, 0, true);
        let (_window_b, _pane_b) = spawn_bare_window(&mut app, 1, false);
        app.world_mut().entity_mut(pane_a).insert(KeyboardFocused);
        app.update();
        let baseline = app.world().resource::<AddCount>().0;

        app.world_mut()
            .write_message(SelectWindowRequest(WindowSelect::Index(9)));
        app.update();

        assert_eq!(
            app.world().resource::<AddCount>().0,
            baseline,
            "an Index request with no matching window must not re-add ActiveMultiplexerWindow"
        );
        assert!(
            app.world()
                .entity(window_a)
                .contains::<ActiveMultiplexerWindow>(),
            "the active window stays active on a no-op index"
        );
        assert!(
            app.world().entity(pane_a).contains::<KeyboardFocused>(),
            "keyboard focus is untouched on a no-op"
        );
    }

    /// Spawns a bare window (as `spawn_bare_window`) with an accompanying
    /// `WindowContainer` + `Node`, the minimal shape
    /// `on_active_window_added` reads. Returns `(window, container)`.
    fn spawn_window_with_container(app: &mut App, index: u32, active: bool) -> (Entity, Entity) {
        let (window, _pane) = spawn_bare_window(app, index, active);
        let container = app
            .world_mut()
            .spawn((WindowContainer { window }, Node::default()))
            .id();
        (window, container)
    }

    #[test]
    fn on_active_window_added_sets_display() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_observer(on_active_window_added);

        let (window_a, container_a) = spawn_window_with_container(&mut app, 0, false);
        let (window_b, container_b) = spawn_window_with_container(&mut app, 1, false);

        app.world_mut()
            .entity_mut(window_b)
            .insert(ActiveMultiplexerWindow);
        app.update();

        assert_eq!(
            app.world().get::<Node>(container_b).unwrap().display,
            Display::Flex,
            "the active window's container is Flex"
        );
        assert_eq!(
            app.world().get::<Node>(container_a).unwrap().display,
            Display::None,
            "the inactive window's container is None"
        );

        app.world_mut()
            .entity_mut(window_b)
            .remove::<ActiveMultiplexerWindow>();
        app.world_mut()
            .entity_mut(window_a)
            .insert(ActiveMultiplexerWindow);
        app.update();

        assert_eq!(
            app.world().get::<Node>(container_a).unwrap().display,
            Display::Flex,
            "after switching, the newly active window's container becomes Flex"
        );
        assert_eq!(
            app.world().get::<Node>(container_b).unwrap().display,
            Display::None,
            "after switching, the previously active window's container becomes None"
        );
    }

    /// Builds an `App` wired with just what `on_new_window` needs: the
    /// `NewWindowRequest` message, `on_new_window` itself, and an
    /// `OrzmaTerminalConfig` resource — mirroring `bootstrap.rs`'s
    /// `build_app`, minus the full UI/bootstrap plugins this test doesn't
    /// exercise.
    fn build_new_window_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(OrzmaTerminalConfig { shell: None });
        app.add_message::<NewWindowRequest>();
        app.add_systems(Update, on_new_window);
        app.world_mut().spawn(WorkspaceContainer);
        app
    }

    /// Spawns an active window with a single pane carrying `PaneCwd(cwd)`,
    /// wired like `ensure_bootstrap` wires the bootstrap window. Returns
    /// `(window, pane)`.
    fn spawn_active_window_with_cwd(app: &mut App, cwd: Option<PathBuf>) -> (Entity, Entity) {
        let world = app.world_mut();
        let pane = world.spawn(PaneCwd(cwd)).id();
        let window = world
            .spawn((
                MultiplexerWindow {
                    index: 0,
                    name: None,
                    active_pane: pane,
                },
                ActiveMultiplexerWindow,
                MultiplexerLayoutComp(MultiplexerLayout::new(pane)),
            ))
            .id();
        world.entity_mut(pane).insert(MultiplexerPane { window });
        (window, pane)
    }

    #[test]
    fn new_window_spawns_second_active_window() {
        let mut app = build_new_window_app();
        let (old_window, old_pane) = spawn_active_window_with_cwd(&mut app, None);

        app.world_mut().write_message(NewWindowRequest);
        app.update();

        let world = app.world_mut();
        let mut windows = world.query::<(Entity, &MultiplexerWindow)>();
        let all: Vec<(Entity, u32)> = windows.iter(world).map(|(e, w)| (e, w.index)).collect();
        assert_eq!(all.len(), 2, "a second window must exist");
        let new_window = all.iter().find(|(e, _)| *e != old_window).unwrap().0;
        assert_eq!(
            all.iter().find(|(e, _)| *e == new_window).unwrap().1,
            1,
            "the new window's index is max+1"
        );
        assert!(
            app.world()
                .entity(new_window)
                .contains::<ActiveMultiplexerWindow>(),
            "the new window becomes active"
        );
        assert!(
            !app.world()
                .entity(old_window)
                .contains::<ActiveMultiplexerWindow>(),
            "the old window loses ActiveMultiplexerWindow"
        );
        let new_pane = app
            .world()
            .get::<MultiplexerWindow>(new_window)
            .unwrap()
            .active_pane;
        assert!(
            app.world().entity(new_pane).contains::<KeyboardFocused>(),
            "the new pane carries KeyboardFocused"
        );
        assert!(
            !app.world().entity(old_pane).contains::<KeyboardFocused>(),
            "the old active pane loses KeyboardFocused"
        );
        let world = app.world_mut();
        let mut containers = world.query_filtered::<(), With<WindowContainer>>();
        assert_eq!(
            containers.iter(world).count(),
            1,
            "the new window gets its own WindowContainer"
        );
    }

    #[test]
    fn new_window_seeds_cwd_from_active_pane() {
        // NOTE: this only exercises that a real, existing seeded cwd does not
        // break the spawn end-to-end; the actual seed VALUE (active pane's
        // cached PaneCwd -> spawn_window's cwd argument) is covered by the
        // pure `seed_cwd_reads_active_panes_cached_cwd` test above, since
        // there is no synchronous, portable way from a unit test to observe
        // which directory the spawned shell process actually chdir'd into.
        let mut app = build_new_window_app();
        let seed_path = std::env::temp_dir();
        spawn_active_window_with_cwd(&mut app, Some(seed_path));

        app.world_mut().write_message(NewWindowRequest);
        app.update();

        let world = app.world_mut();
        let mut panes = world.query_filtered::<Entity, With<MultiplexerPane>>();
        assert_eq!(
            panes.iter(world).count(),
            2,
            "the new window's pane must spawn successfully with a seeded (existing) cwd"
        );
    }
}
