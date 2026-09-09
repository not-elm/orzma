//! One pane as the backend owns it: the terminal plus the geometry the
//! backend last applied to it, and the factory that spawns terminals.

use orzma_tty::prelude::{OrzmaTty, OrzmaTtyResult};
use orzma_tty::{CellPixels, EnvKey, EnvValue, SpawnOptions};
use orzma_vt::prelude::{GridSize, OrzmaVt};
#[cfg(windows)]
use std::path::Path;
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
    /// Resolves the shell now (config → `$SHELL` → the platform default)
    /// so every pane uses the same one.
    pub(crate) fn new(shell: Option<String>, scrollback_rows: usize) -> Self {
        Self {
            shell: resolve_shell(
                shell.as_deref(),
                std::env::var("SHELL").ok().as_deref(),
                shell_exists,
                &default_shell(),
            ),
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

/// Resolves the shell: a non-empty config value wins; otherwise a
/// non-empty `$SHELL` that `exists` accepts; otherwise the platform
/// default.
fn resolve_shell(
    config: Option<&str>,
    env_shell: Option<&str>,
    exists: impl Fn(&str) -> bool,
    platform_default: &str,
) -> String {
    config
        .filter(|s| !s.is_empty())
        .or_else(|| env_shell.filter(|s| !s.is_empty() && exists(s)))
        .unwrap_or(platform_default)
        .to_string()
}

/// The Windows default: `pwsh` (PowerShell 7) if `exists` finds it, then
/// `powershell` (Windows PowerShell), then a non-empty `comspec`, then
/// `cmd.exe`. Bare names are resolved through `PATH` by `portable-pty`
/// at spawn.
#[cfg_attr(
    all(unix, not(test)),
    expect(
        dead_code,
        reason = "the Windows default is unit-tested on every platform"
    )
)]
fn windows_default_shell(exists: impl Fn(&str) -> bool, comspec: Option<&str>) -> String {
    ["pwsh", "powershell"]
        .into_iter()
        .find(|name| exists(name))
        .map(str::to_string)
        .or_else(|| comspec.filter(|s| !s.is_empty()).map(str::to_string))
        .unwrap_or_else(|| "cmd.exe".to_string())
}

/// The Unix default shell.
#[cfg(unix)]
fn default_shell() -> String {
    "/bin/sh".to_string()
}

/// On Unix `$SHELL` is spawned verbatim, as it always was; a bad value
/// fails the spawn and is reported there.
#[cfg(unix)]
fn shell_exists(_shell: &str) -> bool {
    true
}

/// The Windows default shell, probed against the real `PATH`.
#[cfg(windows)]
fn default_shell() -> String {
    windows_default_shell(on_path, std::env::var("COMSPEC").ok().as_deref())
}

#[cfg(windows)]
fn shell_exists(shell: &str) -> bool {
    on_path(shell)
}

/// Whether `name` is a file: as given when it carries a directory, else
/// in some `PATH` entry, as given or with each `PATHEXT` extension
/// appended — the lookup `portable-pty` performs at spawn.
#[cfg(windows)]
fn on_path(name: &str) -> bool {
    if name.contains(['/', '\\']) {
        return Path::new(name).is_file();
    }
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    let extensions: Vec<String> = std::env::var("PATHEXT")
        .ok()
        .map(|v| {
            v.split(';')
                .filter(|e| !e.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_else(|| vec![".EXE".to_string()]);
    std::env::split_paths(&path).any(|dir| {
        dir.join(name).is_file()
            || extensions
                .iter()
                .any(|ext| dir.join(format!("{name}{ext}")).is_file())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts the config → `$SHELL` → platform default precedence.
    ///
    /// Case: a user with `shell = "/bin/fish"` in config on a machine
    /// whose `$SHELL` is zsh.
    #[test]
    fn shell_resolution_prefers_config_then_env_then_default() {
        assert_eq!(
            resolve_shell(Some("/bin/fish"), Some("/bin/zsh"), |_| true, "/bin/sh"),
            "/bin/fish"
        );
        assert_eq!(
            resolve_shell(None, Some("/bin/zsh"), |_| true, "/bin/sh"),
            "/bin/zsh"
        );
        assert_eq!(
            resolve_shell(Some(""), None, |_| true, "/bin/sh"),
            "/bin/sh"
        );
    }

    /// Asserts that a `$SHELL` the probe cannot find falls through to
    /// the platform default rather than being spawned verbatim.
    ///
    /// Case: orzma is launched from Git Bash on Windows, which exports
    /// `SHELL=/usr/bin/bash`, a path that does not exist for Windows.
    #[test]
    fn an_unresolvable_env_shell_falls_through_to_the_default() {
        assert_eq!(
            resolve_shell(None, Some("/usr/bin/bash"), |_| false, "cmd.exe"),
            "cmd.exe"
        );
        assert_eq!(
            resolve_shell(
                None,
                Some("/usr/bin/bash"),
                |s| s == "/usr/bin/bash",
                "cmd.exe"
            ),
            "/usr/bin/bash"
        );
    }

    /// Asserts that the Windows default prefers PowerShell 7, then
    /// Windows PowerShell, then `%COMSPEC%`, then `cmd.exe`.
    ///
    /// Case: a fresh Windows install with no `shell` configured and no
    /// `$SHELL`, with or without PowerShell 7 on `PATH`.
    #[test]
    fn the_windows_default_prefers_pwsh_then_powershell_then_comspec() {
        let both = |name: &str| name == "pwsh" || name == "powershell";
        assert_eq!(
            windows_default_shell(both, Some(r"C:\Windows\system32\cmd.exe")),
            "pwsh"
        );
        let only_powershell = |name: &str| name == "powershell";
        assert_eq!(
            windows_default_shell(only_powershell, Some(r"C:\Windows\system32\cmd.exe")),
            "powershell"
        );
        assert_eq!(
            windows_default_shell(|_| false, Some(r"C:\Windows\system32\cmd.exe")),
            r"C:\Windows\system32\cmd.exe"
        );
        assert_eq!(windows_default_shell(|_| false, Some("")), "cmd.exe");
        assert_eq!(windows_default_shell(|_| false, None), "cmd.exe");
    }
}
