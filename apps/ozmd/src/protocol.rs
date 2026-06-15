//! Wire types for the controller↔page protocol. Emit payloads serialize; call
//! params deserialize. All field names are camelCase on the wire.

use serde::{Deserialize, Serialize};

/// A scroll command sent to the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ScrollAction {
    Down,
    Up,
    HalfDown,
    HalfUp,
    PageDown,
    PageUp,
    Top,
    Bottom,
}

/// A search-navigation direction sent to the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum SearchDir {
    Next,
    Prev,
}

/// The full document content pushed to the page (and returned from `ready`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Content {
    /// Raw Markdown source.
    pub(crate) markdown: String,
    /// Absolute parent directory (string form) of the source file.
    pub(crate) base_dir: String,
}

/// Search-result counts the page reports back (`searchCount` call).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchCount {
    /// Total matches in the document.
    pub(crate) total: usize,
    /// 1-based index of the current match (0 when none).
    pub(crate) current: usize,
}

/// Viewport state the page reports back (`scrollState` call).
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScrollState {
    /// Vertical scroll position as a 0.0..=1.0 ratio.
    pub(crate) ratio: f64,
    /// Index of the `id="h{n}"` anchor nearest the top, or `None`.
    pub(crate) current_heading_index: Option<usize>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn scroll_action_is_camel_case() {
        assert_eq!(serde_json::to_value(ScrollAction::HalfDown).unwrap(), json!("halfDown"));
        assert_eq!(serde_json::to_value(ScrollAction::Top).unwrap(), json!("top"));
    }

    #[test]
    fn content_renames_base_dir() {
        let c = Content { markdown: "# x".into(), base_dir: "/tmp".into() };
        assert_eq!(serde_json::to_value(&c).unwrap(), json!({"markdown": "# x", "baseDir": "/tmp"}));
    }

    #[test]
    fn scroll_state_reads_camel_case_and_null_index() {
        let s: ScrollState = serde_json::from_value(json!({"ratio": 0.5, "currentHeadingIndex": null})).unwrap();
        assert_eq!(s.ratio, 0.5);
        assert_eq!(s.current_heading_index, None);
    }

    #[test]
    fn search_count_reads_camel_case() {
        let s: SearchCount = serde_json::from_value(json!({"total": 12, "current": 3})).unwrap();
        assert_eq!(s, SearchCount { total: 12, current: 3 });
    }
}
