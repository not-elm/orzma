//! Default-mode standalone terminal spawn: the PTY bundle, spawn options, and
//! the shell-override config resource.

use crate::surface::OzmaTerminal;
use bevy::prelude::*;
use ozma_tty_engine::{SpawnOptions, TerminalBundle};
use std::path::PathBuf;

/// Shell override resource.
///
/// `None` means fall back to `$SHELL` at spawn time.
#[derive(Resource)]
pub(crate) struct OzmaTerminalConfig {
    /// Optional shell path. When set, overrides `$SHELL` and `/bin/sh`.
    pub shell: Option<String>,
}

/// Options for spawning a standalone Ozma terminal.
#[derive(Default)]
pub(crate) struct OzmaSpawnOptions {
    /// Shell override; `None` falls back to `$SHELL` then `/bin/sh`.
    pub shell: Option<String>,
    /// Working directory for the PTY; `None` inherits the process cwd.
    pub cwd: Option<PathBuf>,
    /// Extra environment variables for the PTY.
    pub env: Vec<(String, String)>,
}

/// Self-contained spawn bundle for a standalone Ozma terminal: the engine PTY
/// bundle, the `OzmaTerminal` marker, and a default full-screen `Node`. The
/// GPU render bundle is injected by `crate::surface`'s add-observer on
/// insertion.
#[derive(Bundle)]
pub(crate) struct OzmaTerminalBundle {
    terminal: TerminalBundle,
    marker: OzmaTerminal,
    node: Node,
}

impl OzmaTerminalBundle {
    /// Spawns the PTY at a provisional 80x24 (the window-fill resize system
    /// corrects it on the first frame) and returns the bundle. Errors when the
    /// PTY fails to spawn.
    pub(crate) fn spawn(opts: OzmaSpawnOptions) -> anyhow::Result<Self> {
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
            marker: OzmaTerminal,
            node: Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
        })
    }
}

/// Inserts the shell-override config resource read by `ensure_default_mode_ui`.
pub(super) struct DefaultSpawnPlugin {
    /// Shell override from the loaded configs; `None` defers to `$SHELL`.
    pub shell: Option<String>,
}

impl Plugin for DefaultSpawnPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(OzmaTerminalConfig {
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
