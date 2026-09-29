//! The chrome state the controller pushes to the page, and the toast it shows.

use serde::Serialize;
use std::time::{Duration, Instant};

/// How long a toast stays on screen.
const TOAST_LIFETIME: Duration = Duration::from_secs(4);

/// Severity of a toast.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ToastKind {
    /// Something the user asked for failed.
    Error,
    /// A notice that nothing happened.
    Info,
}

/// A transient message and the moment it appeared; the page sees its id,
/// kind, and text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Toast {
    /// Tells this toast apart from every other one, even one with the same
    /// text; the page keeps a toast it dismissed hidden by this id.
    pub(crate) id: u64,
    /// Severity of the message.
    pub(crate) kind: ToastKind,
    /// The message text.
    pub(crate) text: String,
    /// When the message appeared.
    #[serde(skip)]
    pub(crate) shown_at: Instant,
}

/// Everything the page draws around the document that the controller owns
/// (`chrome` event).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Chrome {
    /// File name of the viewed document.
    pub(crate) file_name: String,
    /// Whether the viewed file has been deleted.
    pub(crate) missing: bool,
    /// The message on screen.
    pub(crate) toast: Option<Toast>,
}

impl Toast {
    /// An error message `id` that appeared at `now`.
    pub fn error(id: u64, text: impl Into<String>, now: Instant) -> Self {
        Self {
            id,
            kind: ToastKind::Error,
            text: text.into(),
            shown_at: now,
        }
    }

    /// An informational message `id` that appeared at `now`.
    pub fn info(id: u64, text: impl Into<String>, now: Instant) -> Self {
        Self {
            id,
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
    /// The chrome for the viewed `file_name`, with the deleted flag and the
    /// toast on screen.
    pub fn build(file_name: &str, missing: bool, toast: Option<&Toast>) -> Self {
        Self {
            file_name: file_name.to_owned(),
            missing,
            toast: toast.cloned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Asserts that the chrome of a file with no message names the file and
    /// nothing else.
    ///
    /// Case: the user opens `a.md` and nothing has gone wrong yet.
    #[test]
    fn a_quiet_chrome_names_the_file() {
        assert_eq!(
            Chrome::build("a.md", false, None),
            Chrome {
                file_name: "a.md".into(),
                missing: false,
                toast: None,
            }
        );
    }

    /// Asserts that the chrome serializes with camelCase keys, lowercase
    /// variants, and the toast's id.
    ///
    /// Case: the page receives the chrome after a failed link while the file
    /// has been deleted.
    #[test]
    fn chrome_serializes_to_camel_case() {
        let toast = Toast::error(3, "cannot open b.md", Instant::now());
        let value = serde_json::to_value(Chrome::build("a.md", true, Some(&toast)))
            .expect("chrome serializes");
        assert_eq!(
            value,
            json!({
                "fileName": "a.md",
                "missing": true,
                "toast": { "id": 3, "kind": "error", "text": "cannot open b.md" }
            })
        );
    }

    /// Asserts that a toast expires exactly four seconds after it appeared.
    ///
    /// Case: the user follows a broken link and then leaves the keyboard alone.
    #[test]
    fn a_toast_expires_after_four_seconds() {
        let shown = Instant::now();
        let toast = Toast::info(1, "no previous page", shown);
        assert!(!toast.is_expired(shown + Duration::from_millis(3999)));
        assert!(toast.is_expired(shown + Duration::from_secs(4)));
    }
}
