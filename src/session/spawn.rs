//! Standalone terminal spawn: the PTY bundle, spawn options, and
//! the shell-override config resource.

use crate::surface::OrzmaTerminal;
use bevy::prelude::*;
use orzma_tty_engine::{SpawnOptions, TerminalBundle};
use std::path::PathBuf;

/// Shell override resource.
///
/// `None` means fall back to `$SHELL` at spawn time.
#[derive(Resource)]
pub(crate) struct OrzmaTerminalConfig {
    /// Optional shell path. When set, overrides `$SHELL` and `/bin/sh`.
    pub shell: Option<String>,
}

/// Options for spawning a standalone Orzma terminal.
#[derive(Default)]
pub(crate) struct OrzmaSpawnOptions {
    /// Shell override; `None` falls back to `$SHELL` then `/bin/sh`.
    pub shell: Option<String>,
    /// Working directory for the PTY; `None` inherits the process cwd.
    pub cwd: Option<PathBuf>,
    /// Extra environment variables for the PTY.
    pub env: Vec<(String, String)>,
}

/// Self-contained spawn bundle for a standalone Orzma terminal: the engine PTY
/// bundle, the `OrzmaTerminal` marker, and a default full-screen `Node`. The
/// GPU render bundle is injected by `crate::surface`'s add-observer on
/// insertion.
#[derive(Bundle)]
pub(crate) struct OrzmaTerminalBundle {
    terminal: TerminalBundle,
    marker: OrzmaTerminal,
    node: Node,
}

impl OrzmaTerminalBundle {
    /// Spawns the PTY at a provisional 80x24 (the window-fill resize system
    /// corrects it on the first frame) and returns the bundle. Errors when the
    /// PTY fails to spawn.
    pub(crate) fn spawn(opts: OrzmaSpawnOptions) -> anyhow::Result<Self> {
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
        })
    }
}

/// Full-window absolute layout for the standalone terminal.
fn full_size_node() -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: Val::Px(0.0),
        top: Val::Px(0.0),
        width: Val::Percent(100.0),
        height: Val::Percent(100.0),
        ..default()
    }
}

/// Inserts the shell-override config resource read by `ensure_default_mode_ui`.
pub(super) struct SpawnPlugin {
    /// Shell override from the loaded configs; `None` defers to `$SHELL`.
    pub shell: Option<String>,
}

impl Plugin for SpawnPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(OrzmaTerminalConfig {
            shell: self.shell.clone(),
        });
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
