//! Configuration for the tmux control-mode backend.

use serde::{Deserialize, Serialize};

/// tmux backend settings.
///
/// The control connection is now established by adopting the user's own
/// `tmux -CC` process rather than spawning one, so this table no longer carries
/// a binary path or socket name. It is retained (empty) as a stable, reserved
/// `[tmux]` section: it still parses and, under `deny_unknown_fields`, rejects
/// stale keys (`program`, `socket_name`, `auto_connect`) so old configs fail
/// loudly instead of being silently ignored.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct TmuxConfig {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deprecated_program_now_errors() {
        assert!(
            toml::from_str::<TmuxConfig>(r#"program = "/opt/tmux""#).is_err(),
            "program is removed; deny_unknown_fields must reject it"
        );
    }

    #[test]
    fn deprecated_socket_name_now_errors() {
        assert!(
            toml::from_str::<TmuxConfig>(r#"socket_name = "work""#).is_err(),
            "socket_name is removed; deny_unknown_fields must reject it"
        );
    }

    #[test]
    fn deprecated_auto_connect_now_errors() {
        assert!(
            toml::from_str::<TmuxConfig>("auto_connect = true").is_err(),
            "auto_connect is removed; deny_unknown_fields must reject it"
        );
    }

    #[test]
    fn empty_is_default() {
        let c: TmuxConfig = toml::from_str("").unwrap();
        assert_eq!(c, TmuxConfig::default());
    }
}
