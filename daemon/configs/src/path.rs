//! Resolves which file `OzmuxConfigs::load` should read. Wraps env access
//! behind a trait so tests can substitute a deterministic implementation
//! without mutating process-wide environment variables.

use crate::OzmuxConfigsError;
use crate::OzmuxConfigsResult;
use std::path::{Path, PathBuf};

pub(crate) const ENV_OZMUX_CONFIG: &str = "OZMUX_CONFIG";
pub(crate) const ENV_XDG_CONFIG_HOME: &str = "XDG_CONFIG_HOME";
const CONFIG_REL_PATH: &str = "ozmux/config.toml";
const HOME_CONFIG_DIR: &str = ".config";

/// Abstraction over the environment lookups `resolve_config_path` performs.
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

/// Expands a leading `~` or `~/` in `path` to the home directory.
///
/// Returns:
/// - `Some(path)` unchanged if `path` does not start with `~`.
/// - `Some(home)` if `path` is exactly `~` and `env.home_dir()` is `Some`.
/// - `Some(home.join(rest))` if `path` starts with `~/` and home is `Some`.
/// - `None` if the path starts with `~` but home is `None`, or if the path
///   starts with `~<name>` (other-user form — unsupported).
pub fn expand_user_path(path: &Path, env: &dyn Env) -> Option<PathBuf> {
    let s = path.to_string_lossy();
    if !s.starts_with('~') {
        return Some(path.to_path_buf());
    }
    let home = env.home_dir()?;
    if s == "~" {
        return Some(home);
    }
    let rest = s.strip_prefix("~/")?;
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
}
