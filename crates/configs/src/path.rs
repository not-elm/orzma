//! Resolves which file `OzmuxConfigs::load` should read. Wraps env access
//! behind a trait so tests can substitute a deterministic implementation
//! without mutating process-wide environment variables.

use crate::OzmuxConfigsError;
use crate::OzmuxConfigsResult;
use std::path::{Path, PathBuf};

pub(crate) const ENV_OZMUX_CONFIG: &str = "OZMUX_CONFIG";
pub(crate) const ENV_XDG_CONFIG_HOME: &str = "XDG_CONFIG_HOME";
const CONFIG_REL_PATH: &str = "ozmux/config.toml";
const EXTENSIONS_REL_PATH: &str = "ozmux/extensions";
const PLUGINS_REL_PATH: &str = "ozmux/plugins";
const HOME_CONFIG_DIR: &str = ".config";

/// Abstraction over environment lookups used to resolve user-specified
/// paths. Lets callers (notably `expand_user_path`) substitute the
/// process environment in tests with deterministic fakes.
///
/// External callers should construct or pass `SystemEnv` for production
/// use; the trait's `var` / `home_dir` surface is intentionally narrow
/// so test implementations stay simple. The crate-internal
/// `resolve_config_path` is the other consumer but is `pub(crate)` and
/// not part of the external API.
pub trait Env {
    /// Returns the value of `key`, treating an empty string as unset.
    fn var(&self, key: &str) -> Option<String>;
    /// Returns the user's home directory, if known.
    fn home_dir(&self) -> Option<PathBuf>;
}

/// Production `Env` implementation that delegates to `std::env` and `dirs`.
pub struct SystemEnv;

impl Env for SystemEnv {
    fn var(&self, key: &str) -> Option<String> {
        std::env::var(key).ok().filter(|s| !s.is_empty())
    }
    fn home_dir(&self) -> Option<PathBuf> {
        dirs::home_dir()
    }
}

/// Returns the path that `OzmuxConfigs::load` should read.
///
/// Precedence: `$OZMUX_CONFIG` → `$XDG_CONFIG_HOME/ozmux/config.toml` →
/// `<home_dir>/.config/ozmux/config.toml`. Returns `HomeDirNotFound` only
/// when all three lookups fail.
pub(crate) fn resolve_config_path(env: &dyn Env) -> OzmuxConfigsResult<PathBuf> {
    if let Some(p) = env.var(ENV_OZMUX_CONFIG) {
        return Ok(PathBuf::from(p));
    }
    if let Some(xdg) = env.var(ENV_XDG_CONFIG_HOME) {
        return Ok(PathBuf::from(xdg).join(CONFIG_REL_PATH));
    }
    if let Some(home) = env.home_dir() {
        return Ok(home.join(HOME_CONFIG_DIR).join(CONFIG_REL_PATH));
    }
    Err(OzmuxConfigsError::HomeDirNotFound)
}

/// Returns the directory that ozmux scans for user-installed extensions.
///
/// Precedence: `$XDG_CONFIG_HOME/ozmux/extensions` →
/// `<home_dir>/.config/ozmux/extensions`. Returns `HomeDirNotFound` only
/// when both lookups fail. `$OZMUX_CONFIG` is intentionally not consulted
/// because it points to a config file, not a directory.
pub fn extensions_dir(env: &dyn Env) -> OzmuxConfigsResult<PathBuf> {
    if let Some(xdg) = env.var(ENV_XDG_CONFIG_HOME) {
        return Ok(PathBuf::from(xdg).join(EXTENSIONS_REL_PATH));
    }
    if let Some(home) = env.home_dir() {
        return Ok(home.join(HOME_CONFIG_DIR).join(EXTENSIONS_REL_PATH));
    }
    Err(OzmuxConfigsError::HomeDirNotFound)
}

/// Resolves the user plugins directory (`$XDG_CONFIG_HOME/ozmux/plugins` or
/// `~/.config/ozmux/plugins`).
///
/// Precedence: `$XDG_CONFIG_HOME/ozmux/plugins` →
/// `<home_dir>/.config/ozmux/plugins`. Returns `HomeDirNotFound` only
/// when both lookups fail. `$OZMUX_CONFIG` is intentionally not consulted
/// because it points to a config file, not a directory.
pub fn plugins_dir(env: &dyn Env) -> OzmuxConfigsResult<PathBuf> {
    if let Some(xdg) = env.var(ENV_XDG_CONFIG_HOME) {
        return Ok(PathBuf::from(xdg).join(PLUGINS_REL_PATH));
    }
    if let Some(home) = env.home_dir() {
        return Ok(home.join(HOME_CONFIG_DIR).join(PLUGINS_REL_PATH));
    }
    Err(OzmuxConfigsError::HomeDirNotFound)
}

/// Expands a leading `~` or `~/` (and `~\\` on Windows) in `path` to
/// the home directory.
///
/// Returns:
/// - `Some(path)` unchanged if `path` does not start with `~`.
/// - `Some(home)` if `path` is exactly `~` and `env.home_dir()` is `Some`.
/// - `Some(home.join(rest))` if `path` starts with `~/` (or `~\` on
///   Windows) and home is `Some`.
/// - `None` if the path starts with `~` but home is `None`, the path
///   starts with `~<name>` (other-user form — unsupported), or the
///   path contains non-UTF-8 bytes that prevent reliable prefix
///   handling.
pub fn expand_user_path(path: &Path, env: &dyn Env) -> Option<PathBuf> {
    // NOTE: require valid UTF-8 for the tilde-prefix path. to_string_lossy()
    // would silently substitute U+FFFD for non-UTF-8 bytes, producing a
    // mangled join target that doesn't exist on disk. Refusing to expand
    // surfaces the misconfig via the caller's "expansion failed" warn.
    let s = path.to_str()?;
    if !s.starts_with('~') {
        return Some(path.to_path_buf());
    }
    let home = env.home_dir()?;
    if s == "~" {
        return Some(home);
    }
    let rest = s
        .strip_prefix("~/")
        // NOTE: also accept `~\` on Windows so a TOML path like
        // `~\fonts\Foo.ttf` (native separator) expands correctly.
        .or_else(|| {
            if cfg!(windows) {
                s.strip_prefix("~\\")
            } else {
                None
            }
        })?;
    Some(home.join(rest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::Path;

    struct FakeEnv {
        vars: HashMap<String, String>,
        home: Option<PathBuf>,
    }

    impl Env for FakeEnv {
        fn var(&self, key: &str) -> Option<String> {
            self.vars.get(key).cloned().filter(|s| !s.is_empty())
        }
        fn home_dir(&self) -> Option<PathBuf> {
            self.home.clone()
        }
    }

    #[test]
    fn ozmux_config_takes_precedence() {
        let env = FakeEnv {
            vars: HashMap::from([
                ("OZMUX_CONFIG".into(), "/tmp/x.toml".into()),
                ("XDG_CONFIG_HOME".into(), "/should/be/ignored".into()),
            ]),
            home: Some(PathBuf::from("/should/be/ignored/home")),
        };
        assert_eq!(
            resolve_config_path(&env).unwrap(),
            PathBuf::from("/tmp/x.toml")
        );
    }

    #[test]
    fn xdg_used_when_ozmux_config_absent() {
        let env = FakeEnv {
            vars: HashMap::from([("XDG_CONFIG_HOME".into(), "/cfg".into())]),
            home: Some(PathBuf::from("/home/u")),
        };
        assert_eq!(
            resolve_config_path(&env).unwrap(),
            PathBuf::from("/cfg/ozmux/config.toml")
        );
    }

    #[test]
    fn home_fallback_when_xdg_absent() {
        let env = FakeEnv {
            vars: HashMap::new(),
            home: Some(PathBuf::from("/home/u")),
        };
        assert_eq!(
            resolve_config_path(&env).unwrap(),
            PathBuf::from("/home/u/.config/ozmux/config.toml")
        );
    }

    #[test]
    fn empty_xdg_is_ignored() {
        let env = FakeEnv {
            vars: HashMap::from([("XDG_CONFIG_HOME".into(), "".into())]),
            home: Some(PathBuf::from("/home/u")),
        };
        assert_eq!(
            resolve_config_path(&env).unwrap(),
            PathBuf::from("/home/u/.config/ozmux/config.toml")
        );
    }

    #[test]
    fn home_dir_not_found_when_all_absent() {
        let env = FakeEnv {
            vars: HashMap::new(),
            home: None,
        };
        let err = resolve_config_path(&env).unwrap_err();
        assert!(matches!(err, OzmuxConfigsError::HomeDirNotFound));
    }

    #[test]
    fn extensions_dir_uses_xdg_when_set() {
        let env = FakeEnv {
            vars: HashMap::from([("XDG_CONFIG_HOME".into(), "/tmp/foo".into())]),
            home: Some(PathBuf::from("/home/u")),
        };
        assert_eq!(
            extensions_dir(&env).unwrap(),
            PathBuf::from("/tmp/foo/ozmux/extensions")
        );
    }

    #[test]
    fn extensions_dir_falls_back_to_home_config() {
        let env = FakeEnv {
            vars: HashMap::new(),
            home: Some(PathBuf::from("/home/u")),
        };
        assert_eq!(
            extensions_dir(&env).unwrap(),
            PathBuf::from("/home/u/.config/ozmux/extensions")
        );
    }

    #[test]
    fn extensions_dir_ignores_ozmux_config_var() {
        let env = FakeEnv {
            vars: HashMap::from([("OZMUX_CONFIG".into(), "/tmp/x.toml".into())]),
            home: Some(PathBuf::from("/home/u")),
        };
        assert_eq!(
            extensions_dir(&env).unwrap(),
            PathBuf::from("/home/u/.config/ozmux/extensions"),
            "OZMUX_CONFIG points to a file, not a directory, and must not affect extensions_dir"
        );
    }

    #[test]
    fn extensions_dir_errors_when_no_xdg_and_no_home() {
        let env = FakeEnv {
            vars: HashMap::new(),
            home: None,
        };
        assert!(matches!(
            extensions_dir(&env).unwrap_err(),
            OzmuxConfigsError::HomeDirNotFound
        ));
    }

    #[test]
    fn plugins_dir_uses_xdg_when_set() {
        let env = FakeEnv {
            vars: HashMap::from([("XDG_CONFIG_HOME".into(), "/xdg".into())]),
            home: Some(PathBuf::from("/home/u")),
        };
        assert_eq!(
            plugins_dir(&env).unwrap(),
            PathBuf::from("/xdg/ozmux/plugins")
        );
    }

    #[test]
    fn plugins_dir_falls_back_to_home_config() {
        let env = FakeEnv {
            vars: HashMap::new(),
            home: Some(PathBuf::from("/home/u")),
        };
        assert_eq!(
            plugins_dir(&env).unwrap(),
            PathBuf::from("/home/u/.config/ozmux/plugins")
        );
    }

    #[test]
    fn expand_user_path_passes_through_absolute_path() {
        let env = FakeEnv {
            vars: HashMap::new(),
            home: Some(PathBuf::from("/home/u")),
        };
        let expanded = expand_user_path(Path::new("/etc/fonts/Iosevka.ttf"), &env);
        assert_eq!(expanded, Some(PathBuf::from("/etc/fonts/Iosevka.ttf")));
    }

    #[test]
    fn expand_user_path_substitutes_tilde_with_home() {
        let env = FakeEnv {
            vars: HashMap::new(),
            home: Some(PathBuf::from("/home/u")),
        };
        let expanded = expand_user_path(Path::new("~/.fonts/Iosevka.ttf"), &env);
        assert_eq!(expanded, Some(PathBuf::from("/home/u/.fonts/Iosevka.ttf")));
    }

    #[test]
    fn expand_user_path_bare_tilde_resolves_to_home() {
        let env = FakeEnv {
            vars: HashMap::new(),
            home: Some(PathBuf::from("/home/u")),
        };
        let expanded = expand_user_path(Path::new("~"), &env);
        assert_eq!(expanded, Some(PathBuf::from("/home/u")));
    }

    #[test]
    fn expand_user_path_returns_none_when_tilde_with_no_home_dir() {
        let env = FakeEnv {
            vars: HashMap::new(),
            home: None,
        };
        let expanded = expand_user_path(Path::new("~/.fonts/Iosevka.ttf"), &env);
        assert_eq!(expanded, None);
    }

    #[test]
    fn expand_user_path_returns_none_for_other_user_tilde() {
        let env = FakeEnv {
            vars: HashMap::new(),
            home: Some(PathBuf::from("/home/u")),
        };
        let expanded = expand_user_path(Path::new("~bob/.fonts/Iosevka.ttf"), &env);
        assert_eq!(expanded, None, "~user/... form is not supported");
    }

    #[cfg(unix)]
    #[test]
    fn expand_user_path_returns_none_for_non_utf8_tilde_path() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let env = FakeEnv {
            vars: HashMap::new(),
            home: Some(PathBuf::from("/home/u")),
        };
        // Invalid UTF-8 (0xFF 0xFE) embedded after the ~/ prefix.
        let bytes: Vec<u8> = b"~/\xff\xfe.ttf".to_vec();
        let path = Path::new(OsStr::from_bytes(&bytes));
        let expanded = expand_user_path(path, &env);
        assert_eq!(
            expanded, None,
            "non-UTF-8 tilde-prefixed paths must refuse to expand rather than silently mangle"
        );
    }

    #[cfg(unix)]
    #[test]
    fn expand_user_path_passes_through_non_utf8_path_without_tilde() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let env = FakeEnv {
            vars: HashMap::new(),
            home: Some(PathBuf::from("/home/u")),
        };
        // Non-UTF-8 path WITHOUT a tilde prefix should still pass through
        // (we only care about tilde expansion correctness; absolute paths
        // with weird bytes are handed directly to std::fs::read).
        let bytes: Vec<u8> = b"/etc/\xff\xfe.ttf".to_vec();
        let path = Path::new(OsStr::from_bytes(&bytes));
        let expanded = expand_user_path(path, &env);
        // Refuses to expand because to_str() returns None even for
        // non-tilde paths. This is a behavior change from the previous
        // to_string_lossy() implementation, but it's the safe default:
        // non-UTF-8 paths now fail loud instead of silently mangling.
        assert_eq!(
            expanded, None,
            "non-UTF-8 paths refuse expansion regardless of prefix"
        );
    }

    #[cfg(windows)]
    #[test]
    fn expand_user_path_accepts_windows_backslash_after_tilde() {
        let env = FakeEnv {
            vars: HashMap::new(),
            home: Some(PathBuf::from("C:\\Users\\u")),
        };
        let expanded = expand_user_path(Path::new("~\\fonts\\Iosevka.ttf"), &env);
        assert_eq!(
            expanded,
            Some(PathBuf::from("C:\\Users\\u\\fonts\\Iosevka.ttf"))
        );
    }
}
