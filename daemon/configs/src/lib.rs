//! Config loader for ozmux. Reads `~/.config/ozmux/config.toml`
//! (or `$OZMUX_CONFIG` / `$XDG_CONFIG_HOME` overrides) and resolves it
//! against built-in defaults.

#![warn(missing_docs)]

use crate::shortcuts::{Action, Binding, Key, KeyChord, Modifiers, Prefix, Shortcuts};
use crate::theme::Theme;

pub mod error;
pub mod shortcuts;
pub mod theme;
pub(crate) mod raw;

pub use error::{OzmuxConfigsError, OzmuxConfigsResult};

/// Fully-resolved ozmux configuration.
#[derive(Clone, Debug)]
pub struct OzmuxConfigs {
    /// Shortcut configuration.
    pub shortcuts: Shortcuts,
    /// Theme configuration.
    pub theme: Theme,
}

impl Default for OzmuxConfigs {
    fn default() -> Self {
        Self {
            shortcuts: Shortcuts {
                prefix: Prefix {
                    chord: KeyChord {
                        key: Key::Char('b'),
                        modifiers: Modifiers {
                            ctrl: true,
                            ..Default::default()
                        },
                    },
                    timeout_ms: 2000,
                },
                bindings: vec![Binding {
                    chord: KeyChord {
                        key: Key::Char('x'),
                        modifiers: Modifiers::default(),
                    },
                    action: Action::ClosePane,
                }],
            },
            theme: Theme {
                background: "#1a1b26".into(),
                foreground: "#c0caf5".into(),
                accent: "#414868".into(),
                border: "#414868".into(),
                destructive: "#f7768e".into(),
            },
        }
    }
}
