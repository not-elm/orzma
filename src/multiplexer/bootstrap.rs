//! Multiplexer bootstrap: on startup, spawns the initial window (index 0) with a
//! single pane under the workspace container, and holds the shell-override config
//! resource the pane PTY spawn reads.

use crate::multiplexer::layout::MultiplexerLayout;
use crate::multiplexer::pane::PanePlugin;
use crate::multiplexer::pane::exit::ExitPlugin;
use crate::multiplexer::pane::layout::LayoutPlugin;
use crate::multiplexer::pane::spawn::{
    MultiplexerPaneBundle, MultiplexerPaneSpawnOptions, PaneCwdPlugin, insert_spawned_pane,
    spawn_pane_container,
};
use crate::multiplexer::window::{
    ActiveMultiplexerWindow, MultiplexerLayoutComp, MultiplexerWindow, WindowPlugin,
};
use crate::ui::multiplexer::WorkspaceContainer;
use bevy::prelude::*;
use orzma_webview::ControlPlaneHandle;

/// Shell override resource, read by the bootstrap pane spawn and by
/// `crate::multiplexer::pane`'s split-pane spawn.
///
/// `None` means fall back to `$SHELL` (then `/bin/sh`) at spawn time.
#[derive(Resource)]
pub(in crate::multiplexer) struct OrzmaTerminalConfig {
    pub(in crate::multiplexer) shell: Option<String>,
}

/// Per-window container node hosting that window's pane containers; one per
/// window, and — for window 0 — the bootstrap gate marker (spawned before the
/// PTY, never despawned on error). `window` links the container back to its
/// window entity for the window-switch/layout systems in later PR-1 tasks.
///
/// `pub(crate)` (not `pub(super)`): `crate::ui::multiplexer::divider_handle`
/// reads it to find the active window's container the same way
/// `crate::multiplexer::pane::layout::apply_layout` does.
#[derive(Component)]
pub(crate) struct WindowContainer {
    pub(crate) window: Entity,
}

/// Aggregates the multiplexer's PTY-side lifecycle: the cwd cache, the shell
/// config resource, and the one-window/one-pane bootstrap. The UI subtree is
/// registered separately by `crate::ui::multiplexer::MultiplexerUiPlugin`.
pub(crate) struct MultiplexerPlugin {
    /// Shell override forwarded into `OrzmaTerminalConfig`; `None` defers to `$SHELL`.
    pub shell: Option<String>,
}

impl Plugin for MultiplexerPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(OrzmaTerminalConfig {
            shell: self.shell.clone(),
        })
        .add_plugins((
            PaneCwdPlugin,
            LayoutPlugin,
            ExitPlugin,
            WindowPlugin,
            PanePlugin,
        ))
        .add_systems(
            Update,
            ensure_bootstrap.run_if(
                not(any_with_component::<WindowContainer>)
                    .and_then(any_with_component::<WorkspaceContainer>),
            ),
        );
    }
}

fn ensure_bootstrap(
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
    workspace: Query<Entity, With<WorkspaceContainer>>,
    config: Res<OrzmaTerminalConfig>,
    control: Option<Res<ControlPlaneHandle>>,
) {
    let Ok(workspace) = workspace.single() else {
        return;
    };
    // NOTE: spawn the per-window container BEFORE attempting the PTY. The run
    // condition gates on `WindowContainer` being absent; the error path must NOT
    // despawn it, or a failed spawn leaves the gate true and this Update system
    // re-fires every frame — re-attempting the PTY and re-writing AppExit.
    // Spawning the container first makes a failure a single attempt.
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
    let pane_container = spawn_pane_container(&mut commands, window_container);
    let pane = commands.spawn_empty().id();
    let env = control
        .as_deref()
        .map(|c| c.surface_env(pane).to_vec())
        .unwrap_or_default();
    match MultiplexerPaneBundle::spawn(MultiplexerPaneSpawnOptions {
        shell: config.shell.clone(),
        env,
        ..default()
    }) {
        Ok(bundle) => {
            insert_spawned_pane(
                &mut commands,
                pane,
                window,
                pane_container,
                bundle,
                control.as_deref(),
            );
            commands.entity(window).insert((
                MultiplexerWindow {
                    index: 0,
                    name: None,
                    active_pane: pane,
                },
                ActiveMultiplexerWindow,
                MultiplexerLayoutComp(MultiplexerLayout::new(pane)),
            ));
        }
        Err(e) => {
            // NOTE: keep window_container / pane_container so the
            // WindowContainer gate stays satisfied (single attempt). Despawn
            // only the un-filled id placeholders.
            commands.entity(pane).despawn();
            commands.entity(window).despawn();
            tracing::error!(?e, "failed to spawn multiplexer bootstrap pane");
            exit.write(AppExit::Success);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multiplexer::pane::MultiplexerPane;
    use crate::ui::UiRoot;
    use crate::ui::multiplexer::MultiplexerUiPlugin;
    use orzma_webview::TokenRegistry;
    use std::path::PathBuf;

    fn build_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.world_mut().spawn((Node::default(), UiRoot));
        app.add_plugins(MultiplexerPlugin { shell: None });
        app.add_plugins(MultiplexerUiPlugin);
        app
    }

    #[test]
    fn bootstrap_spawns_one_window_one_pane() {
        let mut app = build_app();
        // Two updates: the first spawns the UI root + WorkspaceContainer (its
        // commands flush at frame end); the second runs the bootstrap, whose gate
        // requires WorkspaceContainer. At runtime this is a one-frame delay.
        app.update();
        app.update();
        let world = app.world_mut();
        let mut windows = world.query_filtered::<(), With<MultiplexerWindow>>();
        assert_eq!(windows.iter(world).count(), 1, "exactly one window");
        let world = app.world_mut();
        let mut panes = world.query_filtered::<(), With<MultiplexerPane>>();
        assert_eq!(panes.iter(world).count(), 1, "exactly one pane");
    }

    #[test]
    fn bootstrap_pane_container_is_absolutely_positioned_at_origin() {
        let mut app = build_app();
        app.update();
        app.update();

        let world = app.world_mut();
        let mut panes = world.query_filtered::<Entity, With<MultiplexerPane>>();
        let pane = panes.single(world).expect("bootstrap pane spawned");
        let container = world.entity(pane).get::<ChildOf>().unwrap().parent();
        let node = world.entity(container).get::<Node>().unwrap();
        assert_eq!(
            node.position_type,
            PositionType::Absolute,
            "a pane container must be absolutely positioned: as a Relative flex \
             child, sibling containers flex-share the window container's area, \
             displacing the pane rects (window-container coordinates) resolved \
             against them"
        );
        assert_eq!(
            node.left,
            Val::Px(0.0),
            "a pane container must pin to the window container's origin"
        );
        assert_eq!(
            node.top,
            Val::Px(0.0),
            "a pane container must pin to the window container's origin"
        );
    }

    #[test]
    fn bootstrap_is_single_attempt() {
        let mut app = build_app();
        app.update();
        app.update();
        app.update();
        let world = app.world_mut();
        let mut containers = world.query_filtered::<(), With<WindowContainer>>();
        assert_eq!(
            containers.iter(world).count(),
            1,
            "the gate must keep the bootstrap to a single window container"
        );
    }

    #[test]
    fn bootstrap_pane_registers_a_resolvable_webview_token() {
        let mut app = build_app();
        let tokens = TokenRegistry::default();
        app.world_mut().insert_resource(ControlPlaneHandle {
            sock_path: PathBuf::from("/tmp/ctl.sock"),
            tokens: tokens.clone(),
        });
        app.update();
        app.update();

        let pane = {
            let world = app.world_mut();
            world
                .query_filtered::<Entity, With<MultiplexerPane>>()
                .single(world)
                .expect("bootstrap pane spawned")
        };
        let token = format!("orzma:{}", pane.to_bits());
        assert_eq!(
            tokens.resolve(&token),
            Some(pane),
            "the bootstrap pane's $ORZMA_TOKEN must resolve to its own surface entity"
        );
    }

    #[test]
    fn config_shell_forwards_to_orzma_terminal_config() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(MultiplexerPlugin {
            shell: Some("/bin/fish".into()),
        });
        assert_eq!(
            app.world()
                .resource::<OrzmaTerminalConfig>()
                .shell
                .as_deref(),
            Some("/bin/fish"),
            "MultiplexerPlugin must forward shell into OrzmaTerminalConfig",
        );
    }
}
