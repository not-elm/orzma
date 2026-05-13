//! Built-in default values for `OzmuxConfigs`, `Shortcuts`, and `Theme`.
//! These are returned when the config file is absent and act as the merge
//! baseline when it is present.

use crate::OzmuxConfigs;
use crate::shortcuts::{Action, Binding, Key, KeyChord, Modifiers, Prefix, Shortcuts};
use crate::theme::Theme;

#[expect(clippy::derivable_impls)]
impl Default for OzmuxConfigs {
    fn default() -> Self {
        Self {
            shortcuts: Shortcuts::default(),
            theme: Theme::default(),
        }
    }
}

impl Default for Shortcuts {
    fn default() -> Self {
        Self {
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
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        // NOTE: values mirror Layer 1 raw tokens in daemon/frontend/src/styles/theme.css.
        Self {
            background: "#1a1b26".into(),
            foreground: "#c0caf5".into(),
            accent: "#414868".into(),
            border: "#414868".into(),
            destructive: "#f7768e".into(),
        }
    }
}
