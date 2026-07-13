//! Multiplexer pane spawn: the PTY spawn bundle, the shared pane-container /
//! pane-insert helpers every spawn site (`ensure_bootstrap`, `on_split_pane`,
//! `spawn_window`) goes through, and the OSC-7 cwd-cache observer that keeps
//! a pane's `PaneCwd` in sync so a split can seed its sibling's working
//! directory.

use crate::input::focus::KeyboardFocused;
use crate::multiplexer::pane::layout::PaneLastCells;
use crate::multiplexer::pane::{MultiplexerPane, PaneCwd};
use crate::surface::OrzmaTerminal;
use bevy::prelude::*;
use orzma_tty_engine::{SpawnOptions, TerminalBundle, TerminalCurrentDir};
use orzma_webview::ControlPlaneHandle;
use std::path::PathBuf;

/// Options for spawning a multiplexer pane's PTY.
#[derive(Default)]
pub(crate) struct MultiplexerPaneSpawnOptions {
    /// Shell override; `None` falls back to `$SHELL` then `/bin/sh`.
    pub shell: Option<String>,
    /// Working directory for the PTY; `None` inherits the process cwd.
    pub cwd: Option<PathBuf>,
    /// Extra environment variables for the PTY.
    pub env: Vec<(String, String)>,
}

/// Self-contained spawn bundle for a multiplexer pane: the engine PTY bundle,
/// the `OrzmaTerminal` marker, a default full-size `Node`, and the cwd cache.
/// The caller inserts `MultiplexerPane`, `KeyboardFocused`, and `ChildOf` once
/// it knows the owning window. The GPU render bundle is injected by
/// `crate::surface`'s add-observer on insertion.
#[derive(Bundle)]
pub(crate) struct MultiplexerPaneBundle {
    terminal: TerminalBundle,
    marker: OrzmaTerminal,
    node: Node,
    cwd: PaneCwd,
    last_cells: PaneLastCells,
}

impl MultiplexerPaneBundle {
    /// Spawns the PTY at a provisional 80x24 (the layout system corrects it
    /// on the first frame) and returns the bundle. Errors when the PTY fails
    /// to spawn.
    pub(crate) fn spawn(opts: MultiplexerPaneSpawnOptions) -> anyhow::Result<Self> {
        let shell = resolve_shell(
            opts.shell.as_deref(),
            std::env::var("SHELL").ok().as_deref(),
        );
        let terminal = TerminalBundle::spawn_login_shell(SpawnOptions {
            cols: 80,
            rows: 24,
            shell,
            cwd: opts.cwd,
            env: opts.env,
        })?;
        Ok(Self {
            terminal,
            marker: OrzmaTerminal,
            node: full_size_node(),
            cwd: PaneCwd::default(),
            last_cells: PaneLastCells::default(),
        })
    }
}

/// Registers the cwd cache observer.
pub(in crate::multiplexer) struct PaneCwdPlugin;

impl Plugin for PaneCwdPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(cache_pane_cwd);
    }
}

/// Spawns a pane's dedicated container under `parent` and returns it. Each
/// pane gets its OWN container so the exit cascade's per-pane
/// `ChildOf`-despawn (`pane/exit.rs`) never takes a surviving sibling down
/// with it.
pub(in crate::multiplexer) fn spawn_pane_container(
    commands: &mut Commands,
    parent: Entity,
) -> Entity {
    commands
        .spawn((
            Name::new("Pane Container"),
            full_size_node(),
            ChildOf(parent),
        ))
        .id()
}

/// Inserts a successfully spawned pane bundle under its container, gives it
/// keyboard focus, and binds it on the control plane.
///
/// # Invariants
///
/// Call only with a successfully spawned `bundle`: the control-plane gc keys
/// on `RemovedComponents<TerminalHandle>` (never added on the error path),
/// so binding before — or despite — a failed PTY spawn would leak the token.
pub(in crate::multiplexer) fn insert_spawned_pane(
    commands: &mut Commands,
    pane: Entity,
    window: Entity,
    pane_container: Entity,
    bundle: MultiplexerPaneBundle,
    control: Option<&ControlPlaneHandle>,
) {
    commands.entity(pane).insert((
        bundle,
        KeyboardFocused,
        MultiplexerPane { window },
        ChildOf(pane_container),
    ));
    if let Some(c) = control {
        c.bind_surface(pane);
    }
}

/// Absolute full-area `Node` pinned to the parent's origin.
///
/// Used for a pane's terminal surface AND for every pane container
/// (`ensure_bootstrap`, `on_split_pane`, `spawn_window`). Containers must be
/// `PositionType::Absolute`: as Relative flex children, sibling containers
/// would flex-share the window container's area, displacing the pane rects
/// (window-container coordinates) `apply_layout` resolves against them.
pub(in crate::multiplexer) fn full_size_node() -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: Val::Px(0.0),
        top: Val::Px(0.0),
        width: Val::Percent(100.0),
        height: Val::Percent(100.0),
        ..default()
    }
}

/// Observer: caches a pane's OSC-7 cwd so a split can seed its sibling.
fn cache_pane_cwd(ev: On<TerminalCurrentDir>, mut panes: Query<&mut PaneCwd>) {
    if let Ok(mut cwd) = panes.get_mut(ev.event_target()) {
        let next = Some(ev.path.clone());
        if cwd.0 != next {
            cwd.0 = next;
        }
    }
}

/// Resolves the shell path: config → `$SHELL` → `/bin/sh`.
fn resolve_shell(config: Option<&str>, env_shell: Option<&str>) -> String {
    config
        .filter(|s| !s.is_empty())
        .or_else(|| env_shell.filter(|s| !s.is_empty()))
        .unwrap_or("/bin/sh")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc7_updates_pane_cwd() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_observer(cache_pane_cwd);
        let pane = app.world_mut().spawn(PaneCwd::default()).id();
        app.world_mut().trigger(TerminalCurrentDir {
            entity: pane,
            path: PathBuf::from("/tmp/x"),
        });
        app.update();
        let cwd = app.world().entity(pane).get::<PaneCwd>().unwrap();
        assert_eq!(cwd.0.as_deref(), Some(std::path::Path::new("/tmp/x")));
    }

    #[test]
    fn shell_resolution_uses_config() {
        assert_eq!(
            resolve_shell(Some("/bin/fish"), Some("/bin/zsh")),
            "/bin/fish"
        );
    }

    #[test]
    fn shell_resolution_falls_back_to_env() {
        assert_eq!(resolve_shell(None, Some("/bin/zsh")), "/bin/zsh");
    }

    #[test]
    fn shell_resolution_falls_back_to_sh() {
        assert_eq!(resolve_shell(None, None), "/bin/sh");
    }
}
