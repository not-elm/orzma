//! The chrome state the controller pushes to the chrome page.

use crate::app::App;
use crate::keymap::Mode;
use serde::Serialize;

/// Everything the chrome page draws (`chrome` event).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Chrome {
    /// The current mode.
    pub(crate) mode: Mode,
    /// The URL loaded in the page.
    pub(crate) url: String,
    /// The first key of a pending two-key chord.
    pub(crate) pending_key: Option<char>,
    /// The text the address input starts with when the address bar opens.
    pub(crate) seed: String,
    /// Incremented each time the address bar opens.
    pub(crate) address_epoch: u32,
}

impl Chrome {
    /// The chrome `app` shows.
    pub fn build(app: &App) -> Self {
        Self {
            mode: app.mode(),
            url: app.url().to_owned(),
            pending_key: app.pending_key(),
            seed: app.seed().to_owned(),
            address_epoch: app.address_epoch(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::keymap::Action;
    use serde_json::json;

    fn chrome(epoch: u32) -> Chrome {
        Chrome {
            mode: Mode::Address,
            url: "https://example.com/".to_owned(),
            pending_key: Some('g'),
            seed: "https://example.com/".to_owned(),
            address_epoch: epoch,
        }
    }

    /// Asserts that the chrome serializes with camelCase keys and mode names.
    ///
    /// Case: the controller pushes the chrome while the address bar is open
    /// and a `g` chord is pending.
    #[test]
    fn the_chrome_serializes_for_the_page() {
        assert_eq!(
            serde_json::to_value(chrome(2)).expect("the chrome serializes"),
            json!({
                "mode": "address",
                "url": "https://example.com/",
                "pendingKey": "g",
                "seed": "https://example.com/",
                "addressEpoch": 2
            })
        );
    }

    /// Asserts that every mode is sent by the name the chrome page expects.
    ///
    /// Case: the app moves through each mode while the chrome shows the badge.
    #[test]
    fn each_mode_serializes_by_its_page_name() {
        for (mode, name) in [
            (Mode::Normal, "normal"),
            (Mode::Insert, "insert"),
            (Mode::Hint, "hint"),
            (Mode::Address, "address"),
            (Mode::Help, "help"),
        ] {
            assert_eq!(
                serde_json::to_value(mode).expect("a mode serializes"),
                json!(name)
            );
        }
    }

    /// Asserts that the chrome reflects the app's mode, URL, pending key, seed,
    /// and epoch.
    ///
    /// Case: the user presses `g`, then `o` to open the address bar.
    #[test]
    fn the_chrome_reflects_the_app() {
        let mut app = App::new("https://example.com/".to_owned());
        app.on_action(Action::Prefix('g'));
        assert_eq!(Chrome::build(&app).pending_key, Some('g'));
        app.on_action(Action::OpenAddress);
        let expected = Chrome {
            pending_key: None,
            ..chrome(1)
        };
        assert_eq!(Chrome::build(&app), expected);
    }
}
