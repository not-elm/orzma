//! The chrome state the controller pushes to the chrome page.

use crate::app::App;
use crate::keymap::Mode;
use serde::Serialize;

/// The mode the chrome shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ChromeMode {
    /// Browsing with the vim keys.
    Normal,
    /// Typing into the page.
    Insert,
    /// Picking a link hint.
    Hint,
    /// Typing into the address bar.
    Address,
    /// Reading the help.
    Help,
}

/// Everything the chrome page draws (`chrome` event).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Chrome {
    /// The current mode.
    pub(crate) mode: ChromeMode,
    /// The URL loaded in the page.
    pub(crate) url: String,
    /// The first key of a pending two-key chord.
    pub(crate) pending_key: Option<char>,
    /// The text the address input starts with when the address bar opens.
    pub(crate) seed: String,
    /// Incremented each time the address bar opens.
    pub(crate) address_epoch: u32,
}

/// Remembers the last chrome sent, so an unchanged chrome is not sent again.
#[derive(Debug, Default)]
pub(crate) struct ChromeSync {
    last: Option<Chrome>,
}

impl Chrome {
    /// The chrome `app` shows.
    pub fn build(app: &App) -> Self {
        Self {
            mode: app.mode().into(),
            url: app.url().to_owned(),
            pending_key: app.pending_key(),
            seed: app.seed().to_owned(),
            address_epoch: app.address_epoch(),
        }
    }
}

impl From<Mode> for ChromeMode {
    fn from(mode: Mode) -> Self {
        match mode {
            Mode::Normal => Self::Normal,
            Mode::Insert => Self::Insert,
            Mode::Hint => Self::Hint,
            Mode::Address => Self::Address,
            Mode::Help => Self::Help,
        }
    }
}

impl ChromeSync {
    /// Whether `chrome` differs from the last chrome sent.
    pub fn is_stale(&self, chrome: &Chrome) -> bool {
        self.last.as_ref() != Some(chrome)
    }

    /// Records `chrome` as sent.
    pub fn mark_sent(&mut self, chrome: Chrome) {
        self.last = Some(chrome);
    }

    /// Forgets the last chrome sent, so the next one is sent unconditionally.
    pub fn forget(&mut self) {
        self.last = None;
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
            mode: ChromeMode::Address,
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

    /// Asserts that every mode maps to its chrome mode.
    ///
    /// Case: the app moves through each mode while the chrome shows the badge.
    #[test]
    fn each_mode_maps_to_a_chrome_mode() {
        for (mode, chrome_mode) in [
            (Mode::Normal, ChromeMode::Normal),
            (Mode::Insert, ChromeMode::Insert),
            (Mode::Hint, ChromeMode::Hint),
            (Mode::Address, ChromeMode::Address),
            (Mode::Help, ChromeMode::Help),
        ] {
            assert_eq!(ChromeMode::from(mode), chrome_mode);
        }
    }

    /// Asserts that a chrome is stale until sent, stale again once it changes,
    /// and stale after the sync forgets it.
    ///
    /// Case: the loop pushes the chrome each pass, the address bar reopens,
    /// and the page reloads and asks for the chrome again.
    #[test]
    fn a_chrome_is_sent_once_per_change() {
        let mut sync = ChromeSync::default();
        assert!(sync.is_stale(&chrome(1)));
        sync.mark_sent(chrome(1));
        assert!(!sync.is_stale(&chrome(1)));
        assert!(sync.is_stale(&chrome(2)));
        sync.forget();
        assert!(sync.is_stale(&chrome(1)));
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
