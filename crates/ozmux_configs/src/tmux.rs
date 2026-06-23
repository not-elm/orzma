//! Configuration for the tmux control-mode backend.

use serde::{Deserialize, Serialize};

/// tmux backend settings.
///
/// The control connection is now established by adopting the user's own
/// `tmux -CC` process. This section is reserved for future settings;
/// any unknown keys are rejected under `deny_unknown_fields`.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct TmuxConfig {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_default() {
        let c: TmuxConfig = toml::from_str("").unwrap();
        assert_eq!(c, TmuxConfig::default());
    }
}
