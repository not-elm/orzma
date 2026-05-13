//! Resolves which file `OzmuxConfigs::load` should read. Wraps env access
//! behind a trait so tests can substitute a deterministic implementation
//! without mutating process-wide environment variables.

// `SystemEnv` and `resolve_config_path` are intentionally unused until
// `OzmuxConfigs::load` is wired up in a subsequent task. The `#[expect]`
// will start failing at that point, signalling that the attribute can be removed.
#![cfg_attr(not(test), expect(dead_code, reason = "consumed by OzmuxConfigs::load in a subsequent task"))]

use crate::OzmuxConfigsError;
use crate::OzmuxConfigsResult;
use std::path::PathBuf;

/// Abstraction over the environment lookups `resolve_config_path` performs.
pub(crate) trait Env {
    /// Returns the value of `key`, treating an empty string as unset.
    fn var(&self, key: &str) -> Option<String>;
    /// Returns the user's home directory, if known.
    fn home_dir(&self) -> Option<PathBuf>;
}

/// Production `Env` implementation that delegates to `std::env` and `dirs`.
#[cfg_attr(test, expect(dead_code, reason = "consumed by OzmuxConfigs::load in a subsequent task"))]
pub(crate) struct SystemEnv;

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
    if let Some(p) = env.var("OZMUX_CONFIG") {
        return Ok(PathBuf::from(p));
    }
    if let Some(xdg) = env.var("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(xdg).join("ozmux/config.toml"));
    }
    if let Some(home) = env.home_dir() {
        return Ok(home.join(".config/ozmux/config.toml"));
    }
    Err(OzmuxConfigsError::HomeDirNotFound)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

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
        assert_eq!(resolve_config_path(&env).unwrap(), PathBuf::from("/tmp/x.toml"));
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
}
