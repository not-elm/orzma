//! The chrome state the controller pushes to the page, and the toast it shows.

use crate::app::App;
use serde::Serialize;
use std::time::{Duration, Instant};

/// How long a toast stays on screen.
const TOAST_LIFETIME: Duration = Duration::from_secs(4);

/// Which stage of a search the page shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum SearchStage {
    /// No search box.
    Closed,
    /// A query is being typed in the search box.
    Typing,
    /// Confirmed matches are highlighted.
    Active,
}

/// Severity of a toast.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ToastKind {
    /// Something the user asked for failed.
    Error,
    /// A notice that nothing happened.
    Info,
}

/// A transient message and the moment it appeared; the page sees its kind and text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Toast {
    /// Severity of the message.
    pub(crate) kind: ToastKind,
    /// The message text.
    pub(crate) text: String,
    /// When the message appeared.
    #[serde(skip)]
    pub(crate) shown_at: Instant,
}

/// The outline panel as the page sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OutlineView {
    /// Whether the panel is open.
    pub(crate) open: bool,
    /// Index of the selected `id="h{n}"` heading.
    pub(crate) selected: usize,
}

/// Everything the page draws around the document (`chrome` event).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Chrome {
    /// File name of the viewed document.
    pub(crate) file_name: String,
    /// Whether the viewed file has been deleted.
    pub(crate) missing: bool,
    /// First key of a pending two-key chord.
    pub(crate) pending_key: Option<char>,
    /// The message on screen.
    pub(crate) toast: Option<Toast>,
    /// The outline panel.
    pub(crate) outline: OutlineView,
    /// The search stage.
    pub(crate) search: SearchStage,
}

impl Toast {
    /// An error message that appeared at `now`.
    pub fn error(text: impl Into<String>, now: Instant) -> Self {
        Self {
            kind: ToastKind::Error,
            text: text.into(),
            shown_at: now,
        }
    }

    /// An informational message that appeared at `now`.
    pub fn info(text: impl Into<String>, now: Instant) -> Self {
        Self {
            kind: ToastKind::Info,
            text: text.into(),
            shown_at: now,
        }
    }

    /// Whether the message has been on screen for four seconds or more at `now`.
    pub fn is_expired(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.shown_at) >= TOAST_LIFETIME
    }
}

impl Chrome {
    /// The chrome for `app` viewing `file_name`, with the deleted flag and the toast on screen.
    pub fn build(app: &App, file_name: &str, missing: bool, toast: Option<&Toast>) -> Self {
        Self {
            file_name: file_name.to_owned(),
            missing,
            pending_key: app.pending_key(),
            toast: toast.cloned(),
            outline: OutlineView {
                open: app.outline_open(),
                selected: app.selected(),
            },
            search: app.search_stage(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::Action;
    use serde_json::json;

    /// Asserts that a fresh app yields closed, quiet chrome naming the file.
    ///
    /// Case: the user opens `a.md` and has not pressed any key yet.
    #[test]
    fn a_fresh_app_has_quiet_chrome() {
        let chrome = Chrome::build(&App::default(), "a.md", false, None);
        assert_eq!(
            chrome,
            Chrome {
                file_name: "a.md".into(),
                missing: false,
                pending_key: None,
                toast: None,
                outline: OutlineView {
                    open: false,
                    selected: 0
                },
                search: SearchStage::Closed,
            }
        );
    }

    /// Asserts that the chrome carries the first key of a pending chord.
    ///
    /// Case: the user presses `g` and has not yet pressed the second `g`.
    #[test]
    fn the_pending_chord_key_is_shown() {
        let mut app = App::default();
        app.on_action(Action::Prefix('g'));
        assert_eq!(
            Chrome::build(&app, "a.md", false, None).pending_key,
            Some('g')
        );
    }

    /// Asserts that a search being typed is reported as the typing stage.
    ///
    /// Case: the user presses `/` to start a search.
    #[test]
    fn a_typed_search_is_the_typing_stage() {
        let mut app = App::default();
        app.on_action(Action::EnterSearch);
        assert_eq!(
            Chrome::build(&app, "a.md", false, None).search,
            SearchStage::Typing
        );
    }

    /// Asserts that the chrome reports an open outline and its selection.
    ///
    /// Case: the user presses `o` at the top of a document.
    #[test]
    fn an_open_outline_is_reported() {
        let mut app = App::default();
        app.on_action(Action::ToggleOutline);
        assert_eq!(
            Chrome::build(&app, "a.md", false, None).outline,
            OutlineView {
                open: true,
                selected: 0
            }
        );
    }

    /// Asserts that the chrome serializes with camelCase keys and lowercase variants.
    ///
    /// Case: the page receives the chrome after a failed link while `g` is pending
    /// and the file has been deleted.
    #[test]
    fn chrome_serializes_to_camel_case() {
        let mut app = App::default();
        app.on_action(Action::Prefix('g'));
        let toast = Toast::error("cannot open b.md", Instant::now());
        let value = serde_json::to_value(Chrome::build(&app, "a.md", true, Some(&toast)))
            .expect("chrome serializes");
        assert_eq!(
            value,
            json!({
                "fileName": "a.md",
                "missing": true,
                "pendingKey": "g",
                "toast": { "kind": "error", "text": "cannot open b.md" },
                "outline": { "open": false, "selected": 0 },
                "search": "closed"
            })
        );
    }

    /// Asserts that a toast expires exactly four seconds after it appeared.
    ///
    /// Case: the user follows a broken link and then leaves the keyboard alone.
    #[test]
    fn a_toast_expires_after_four_seconds() {
        let shown = Instant::now();
        let toast = Toast::info("no previous page", shown);
        assert!(!toast.is_expired(shown + Duration::from_millis(3999)));
        assert!(toast.is_expired(shown + Duration::from_secs(4)));
    }
}
