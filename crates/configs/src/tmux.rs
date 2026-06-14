//! Configuration for the tmux control-mode backend.

use serde::{Deserialize, Serialize};

/// tmux backend settings.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct TmuxConfig {
    /// tmux binary to run (looked up on `PATH` unless absolute).
    pub program: String,
    /// Optional named server socket (`tmux -L <name>`); `None` targets the
    /// default server, which is what a normal CLI `tmux` uses.
    pub socket_name: Option<String>,
    /// Whether to connect to tmux automatically at startup.
    pub auto_connect: bool,
}

impl Default for TmuxConfig {
    fn default() -> Self {
        Self {
            program: "tmux".to_string(),
            socket_name: None,
            auto_connect: false,
        }
    }
}

/// Per-field-optional view of `[tmux]` for TOML deserialization.
#[derive(Deserialize, Default, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct TmuxPatch {
    /// Optional `[tmux].program` override.
    pub program: Option<String>,
    /// Optional `[tmux].socket_name` override.
    pub socket_name: Option<String>,
    /// Optional `[tmux].auto_connect` override.
    pub auto_connect: Option<bool>,
}

impl TmuxPatch {
    /// Applies this patch over `base`, keeping `base`'s value where unset.
    pub fn apply_to(self, base: TmuxConfig) -> TmuxConfig {
        TmuxConfig {
            program: self.program.unwrap_or(base.program),
            socket_name: self.socket_name.or(base.socket_name),
            auto_connect: self.auto_connect.unwrap_or(base.auto_connect),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_targets_path_tmux_default_socket_no_autoconnect() {
        let c = TmuxConfig::default();
        assert_eq!(c.program, "tmux");
        assert_eq!(c.socket_name, None);
        assert!(!c.auto_connect);
    }

    #[test]
    fn patch_overrides_set_fields_only() {
        let patched = TmuxPatch {
            program: Some("/opt/tmux".to_string()),
            socket_name: None,
            auto_connect: Some(true),
        }
        .apply_to(TmuxConfig::default());
        assert_eq!(patched.program, "/opt/tmux");
        assert_eq!(patched.socket_name, None);
        assert!(patched.auto_connect);
    }

    #[test]
    fn empty_patch_keeps_base() {
        let patched = TmuxPatch::default().apply_to(TmuxConfig::default());
        assert_eq!(patched, TmuxConfig::default());
    }
}
