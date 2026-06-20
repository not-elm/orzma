//! Configuration for the tmux control-mode backend.

use serde::{Deserialize, Serialize};

/// tmux backend settings.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct TmuxConfig {
    /// tmux binary to run (looked up on `PATH` unless absolute).
    pub program: String,
    /// Optional named server socket (`tmux -L <name>`); `None` targets the
    /// default server, which is what a normal CLI `tmux` uses.
    pub socket_name: Option<String>,
}

impl Default for TmuxConfig {
    fn default() -> Self {
        Self {
            program: "tmux".to_string(),
            socket_name: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_targets_path_tmux_default_socket() {
        let c = TmuxConfig::default();
        assert_eq!(c.program, "tmux");
        assert_eq!(c.socket_name, None);
    }

    #[test]
    fn partial_overrides_program_only() {
        let c: TmuxConfig = toml::from_str(r#"program = "/opt/tmux""#).unwrap();
        assert_eq!(c.program, "/opt/tmux");
        assert_eq!(c.socket_name, None);
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
