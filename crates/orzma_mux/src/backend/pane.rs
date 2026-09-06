//! One pane as the backend owns it: the terminal plus the geometry the
//! backend last applied to it, and the factory that spawns terminals.

use orzma_tty::prelude::{OrzmaTty, OrzmaTtyResult};
use orzma_tty::{CellPixels, EnvKey, EnvValue, SpawnOptions};
use orzma_vt::prelude::{GridSize, OrzmaVt};
use std::path::PathBuf;

/// A live pane.
pub(crate) struct Pane {
    pub(crate) tty: OrzmaTty<OrzmaVt>,
    /// The `(cols, rows, cell_px)` the PTY was last successfully sized to.
    pub(crate) applied: (u16, u16, CellPixels),
    /// The last directory the shell reported through OSC 7, or the
    /// directory the pane was spawned in until it reports one.
    pub(crate) cwd: Option<PathBuf>,
}

/// Spawns terminals for the backend. Abstracted so tests can hand the
/// backend PTY-less terminals.
pub(crate) trait PaneFactory: Send {
    fn spawn(
        &mut self,
        size: GridSize,
        cell_px: CellPixels,
        cwd: Option<PathBuf>,
        env: Vec<(String, String)>,
    ) -> OrzmaTtyResult<OrzmaTty<OrzmaVt>>;
}

/// The production factory: spawns the login shell under a real PTY.
pub(crate) struct ShellFactory {
    shell: String,
    scrollback_rows: usize,
}

impl ShellFactory {
    /// Resolves the shell now (config → `$SHELL` → `/bin/sh`) so every
    /// pane uses the same one.
    pub(crate) fn new(shell: Option<String>, scrollback_rows: usize) -> Self {
        Self {
            shell: resolve_shell(shell.as_deref(), std::env::var("SHELL").ok().as_deref()),
            scrollback_rows,
        }
    }
}

impl PaneFactory for ShellFactory {
    fn spawn(
        &mut self,
        size: GridSize,
        cell_px: CellPixels,
        cwd: Option<PathBuf>,
        env: Vec<(String, String)>,
    ) -> OrzmaTtyResult<OrzmaTty<OrzmaVt>> {
        let vt = OrzmaVt::new(size, self.scrollback_rows);
        OrzmaTty::spawn(
            vt,
            SpawnOptions {
                cols: size.cols,
                rows: size.rows,
                cell_px,
                shell: self.shell.clone(),
                cwd,
                env: env
                    .into_iter()
                    .map(|(k, v)| (EnvKey(k), EnvValue(v)))
                    .collect(),
            },
        )
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

    /// Asserts the config → `$SHELL` → `/bin/sh` precedence.
    ///
    /// Case: a user with `shell = "/bin/fish"` in config on a machine
    /// whose `$SHELL` is zsh.
    #[test]
    fn shell_resolution_prefers_config_then_env_then_sh() {
        assert_eq!(
            resolve_shell(Some("/bin/fish"), Some("/bin/zsh")),
            "/bin/fish"
        );
        assert_eq!(resolve_shell(None, Some("/bin/zsh")), "/bin/zsh");
        assert_eq!(resolve_shell(Some(""), None), "/bin/sh");
    }
}
