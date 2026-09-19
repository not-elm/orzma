//! One pane as the backend owns it, together with the factory that
//! spawns a pane's terminal.

use orzma_tty::prelude::{OrzmaTty, OrzmaTtyResult};
use orzma_tty::{CellPixels, EnvKey, EnvValue, SpawnOptions};
use orzma_vt::prelude::{CursorPolicy, GridSize, OrzmaVt};
#[cfg(windows)]
use std::path::Path;
use std::path::PathBuf;

/// A live pane.
pub(crate) struct Pane {
    pub(crate) tty: OrzmaTty<OrzmaVt>,
    /// The `(cols, rows, cell_px)` the PTY was last successfully sized to.
    pub(crate) applied: (u16, u16, CellPixels),
    /// The last directory the shell reported through OSC 7.
    osc7_cwd: Option<PathBuf>,
    /// The directory the pane's shell was spawned in, when one was given.
    spawn_cwd: Option<PathBuf>,
}

impl Pane {
    /// A pane around `tty`, whose PTY is sized to `applied` and whose shell
    /// was spawned in `spawn_cwd`. No OSC 7 report is recorded yet.
    pub fn new(
        tty: OrzmaTty<OrzmaVt>,
        applied: (u16, u16, CellPixels),
        spawn_cwd: Option<PathBuf>,
    ) -> Self {
        Self {
            tty,
            applied,
            osc7_cwd: None,
            spawn_cwd,
        }
    }

    /// The pane's working directory: the directory the OS reports for its
    /// foreground process or shell, else the directory the shell last
    /// reported through OSC 7, else the directory the pane was spawned in.
    ///
    /// Returns `None` when none of them is known.
    pub fn cwd(&self) -> Option<PathBuf> {
        // TODO: on Windows, prefer the directory the shell reports through
        // OSC 7 or OSC 9;9 over the process directory, because
        // PowerShell's Set-Location does not change the process working
        // directory.
        self.tty
            .process_cwd()
            .or_else(|| self.osc7_cwd.clone())
            .or_else(|| self.spawn_cwd.clone())
    }

    /// Records `path` as the directory the shell last reported through
    /// OSC 7, replacing any earlier report.
    pub fn set_osc7_cwd(&mut self, path: PathBuf) {
        self.osc7_cwd = Some(path);
    }
}

/// Spawns terminals for the backend.
pub(crate) trait PaneFactory: Send {
    fn spawn(
        &mut self,
        size: GridSize,
        cell_px: CellPixels,
        cwd: Option<PathBuf>,
        env: Vec<(String, String)>,
    ) -> OrzmaTtyResult<OrzmaTty<OrzmaVt>>;
}

/// Spawns the resolved shell under a real PTY.
pub(crate) struct ShellFactory {
    shell: String,
    scrollback_rows: usize,
    cursor_policy: CursorPolicy,
}

impl ShellFactory {
    /// Resolves the shell now (config → `$SHELL` → the platform default)
    /// so every pane uses the same one.
    pub(crate) fn new(
        shell: Option<String>,
        scrollback_rows: usize,
        cursor_policy: CursorPolicy,
    ) -> Self {
        Self {
            shell: resolve_shell(
                shell.as_deref(),
                std::env::var("SHELL").ok().as_deref(),
                shell_exists,
                &default_shell(),
            ),
            scrollback_rows,
            cursor_policy,
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
        let vt = OrzmaVt::new(size, self.scrollback_rows).with_cursor_policy(self.cursor_policy);
        OrzmaTty::spawn(
            vt,
            SpawnOptions {
                size,
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
/// `cmd.exe`. Bare names are resolved through `PATH` at spawn.
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

/// On Unix `$SHELL` is spawned verbatim; a bad value fails the spawn
/// and is reported there.
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
/// appended.
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
    use orzma_tty::test_support::CaptureSink;
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    use orzma_vt::prelude::{CursorBlink, CursorShape, TextCursorStyle, Vt};
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    use std::thread;
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    use std::time::{Duration, Instant};
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    use tempfile::TempDir;

    /// A terminal with no process behind it, so the OS reports no
    /// directory for it.
    fn detached_tty() -> OrzmaTty<OrzmaVt> {
        let size = GridSize::new(80, 24).expect("a valid size");
        OrzmaTty::detached(
            OrzmaVt::new(size, 100),
            size,
            Box::new(CaptureSink::default()),
        )
        .expect("a detached terminal")
    }

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

    /// Asserts that a pane reports the directory it was spawned in when the
    /// OS reports no directory for its process and its shell has sent no
    /// OSC 7 report.
    ///
    /// Case: the user splits a pane again before its new shell has printed
    /// its first prompt, while the OS cannot be asked for the pane's
    /// directory.
    #[test]
    fn a_pane_without_an_osc7_report_falls_back_to_its_spawn_directory() {
        let pane = Pane::new(
            detached_tty(),
            (80, 24, CellPixels::default()),
            Some(PathBuf::from("/spawned")),
        );
        assert_eq!(pane.cwd(), Some(PathBuf::from("/spawned")));
    }

    /// Asserts that a directory reported through OSC 7 is reported ahead
    /// of the spawn directory.
    ///
    /// Case: a shell that reports its directory through OSC 7 changes into
    /// a project while the OS cannot be asked for the pane's directory.
    #[test]
    fn an_osc7_report_wins_over_the_spawn_directory() {
        let mut pane = Pane::new(
            detached_tty(),
            (80, 24, CellPixels::default()),
            Some(PathBuf::from("/spawned")),
        );
        pane.set_osc7_cwd(PathBuf::from("/reported"));
        assert_eq!(pane.cwd(), Some(PathBuf::from("/reported")));
    }

    /// Asserts that the directory the OS reports for the pane's process is
    /// reported ahead of an OSC 7 report.
    ///
    /// Case: the shell last reported one directory, and the program now
    /// running in the pane works in another.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn the_process_directory_wins_over_an_osc7_report() {
        let dir = TempDir::new().expect("a temp dir");
        let expected = dir.path().canonicalize().expect("the dir canonicalizes");
        let size = GridSize::new(80, 24).expect("a valid size");
        let tty = ShellFactory::new(Some("/bin/cat".into()), 100, CursorPolicy::default())
            .spawn(
                size,
                CellPixels::default(),
                Some(dir.path().to_path_buf()),
                Vec::new(),
            )
            .expect("cat spawns under a PTY");
        let mut pane = Pane::new(tty, (80, 24, CellPixels::default()), None);
        pane.set_osc7_cwd(PathBuf::from("/reported"));
        let deadline = Instant::now() + Duration::from_secs(10);
        let reported = loop {
            let reported = pane.cwd();
            if reported.as_ref() == Some(&expected) || Instant::now() >= deadline {
                break reported;
            }
            thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(reported, Some(expected));
    }

    /// Asserts that a spawned pane's terminal starts with the factory's
    /// configured cursor style, so a configured caret is in force before
    /// the shell writes anything.
    ///
    /// Case: the user configures a blinking bar and opens a new pane.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn a_spawned_pane_starts_with_the_configured_style() {
        let policy = CursorPolicy {
            initial: TextCursorStyle {
                shape: CursorShape::Bar,
                blink: CursorBlink::Blinking,
            },
        };
        let tty = ShellFactory::new(Some("/bin/cat".into()), 100, policy)
            .spawn(
                GridSize::new(80, 24).expect("a valid size"),
                CellPixels::default(),
                None,
                Vec::new(),
            )
            .expect("cat spawns under a PTY");
        let cursor = tty.vt().modes().text_cursor;
        assert_eq!(cursor.shape, CursorShape::Bar);
        assert_eq!(cursor.blink, CursorBlink::Blinking);
    }
}
