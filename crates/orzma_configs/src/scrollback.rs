//! The `[scrollback]` config section: how much pre-attach tmux history to
//! seed into a pane's local scrollback on attach.

use serde::{Deserialize, Serialize};

/// User-facing `[scrollback]` configuration.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct ScrollbackConfig {
    /// Lines of tmux history to fetch and seed on attach. Clamped by the
    /// engine's fixed scrollback cap (10000) when applied.
    pub seed_lines: usize,
}

impl Default for ScrollbackConfig {
    fn default() -> Self {
        Self { seed_lines: 2000 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_seed_lines_is_2000() {
        assert_eq!(ScrollbackConfig::default().seed_lines, 2000);
    }

    #[test]
    fn parses_seed_lines_from_toml() {
        let cfg: ScrollbackConfig = toml::from_str("seed-lines = 5000").unwrap();
        assert_eq!(cfg.seed_lines, 5000);
    }
}
