//! Configuration for the tmux control-mode backend.

use serde::{Deserialize, Serialize};

/// tmux backend settings.
///
/// The control connection is now established by adopting the user's own
/// `tmux -CC` process rather than spawning one, so this table no longer carries
/// a binary path, socket name, or auto-connect flag. The deprecated fields
/// `program`, `socket_name`, and `auto_connect` are accepted and ignored so
/// existing configs continue to load under `deny_unknown_fields`.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct TmuxConfig {
    /// Deprecated and ignored: the tmux binary is no longer configured here;
    /// the connection adopts the user's own `tmux -CC` process. Accepted so
    /// existing configs carrying it still parse under `deny_unknown_fields`.
    /// Remove after one release.
    #[serde(default, skip_serializing)]
    program: Option<String>,
    /// Deprecated and ignored: the tmux socket name is no longer configured
    /// here; the connection adopts the user's own `tmux -CC` process. Accepted
    /// so existing configs carrying it still parse under `deny_unknown_fields`.
    /// Remove after one release.
    #[serde(default, skip_serializing)]
    socket_name: Option<String>,
    /// Deprecated and ignored: the connection adopts the user's own `tmux -CC`
    /// process, so there is no auto-connect step. Accepted so existing configs
    /// carrying it still parse under `deny_unknown_fields`. Remove after one
    /// release.
    #[serde(default, skip_serializing)]
    auto_connect: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deprecated_program_is_accepted_and_ignored() {
        toml::from_str::<TmuxConfig>(r#"program = "/opt/tmux""#)
            .expect("deprecated program key must parse under deny_unknown_fields");
    }

    #[test]
    fn deprecated_socket_name_is_accepted_and_ignored() {
        toml::from_str::<TmuxConfig>(r#"socket_name = "work""#)
            .expect("deprecated socket_name key must parse under deny_unknown_fields");
    }

    #[test]
    fn deprecated_auto_connect_is_accepted_and_ignored() {
        toml::from_str::<TmuxConfig>("auto_connect = true")
            .expect("deprecated auto_connect key must parse under deny_unknown_fields");
    }

    #[test]
    fn empty_is_default() {
        let c: TmuxConfig = toml::from_str("").unwrap();
        assert_eq!(c, TmuxConfig::default());
    }
}
