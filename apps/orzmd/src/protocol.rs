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

/// A page request to navigate to a local Markdown file (`navigate` event).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NavigateRequest {
    /// The link's raw path (relative or absolute), percent-decoded by the page.
    pub(crate) path: String,
    /// The link's `#fragment`, if any, to scroll to after the document loads.
    #[serde(default)]
    pub(crate) fragment: Option<String>,
}

/// A page request to open an external URL in the system browser (`openExternal`).
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct OpenExternal {
    /// The external URL (`http`/`https`/`mailto`/`tel`).
    pub(crate) url: String,
}

/// A page request to open a local non-Markdown file with the OS default app
/// (`openPath` event).
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct OpenPath {
    /// The link's raw path (relative or absolute), percent-decoded by the page.
    pub(crate) path: String,
}

/// Where the page should scroll after applying a `content` push.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(crate) enum ScrollTo {
    /// Keep the current scroll anchor (initial load / file-change reload).
    Preserve,
    /// Jump to the top (forward navigation with no fragment).
    Top,
    /// Restore a 0.0..=1.0 ratio (back navigation).
    Ratio { ratio: f64 },
    /// Scroll to the slug-id element (forward navigation to `file.md#frag`).
    Slug { slug: String },
}

/// The full document content pushed to the page (and returned from `ready`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Content {
    /// Raw Markdown source.
    pub(crate) markdown: String,
    /// Absolute parent directory (string form) of the source file.
    pub(crate) base_dir: String,
    /// Where the page should scroll after rendering this content.
    pub(crate) scroll_to: ScrollTo,
}

/// A page request to stage local image files referenced by the current
/// document (`stageAssets` call).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StageAssetsRequest {
    /// Percent-decoded, query/fragment-stripped local paths to stage.
    pub(crate) paths: Vec<String>,
}

/// The staged served URLs for a `stageAssets` request, aligned to the request
/// order; an entry is `None` when that path could not be staged.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StageAssetsResponse {
    /// Root-relative served URL (`_local/<token>.<ext>`) per input path.
    pub(crate) urls: Vec<Option<String>>,
}

/// Why the page ended a search that was being typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum SearchCause {
    /// Enter, Escape, or the close button.
    Key,
    /// The search input lost focus.
    Blur,
}

/// Text for the page's search input (`searchType` emit).
#[derive(Debug, Clone, Serialize)]
pub(crate) struct SearchType {
    /// The typed text.
    pub(crate) text: String,
}

/// Viewport state the page reports after a scroll or a render.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScrollState {
    /// Vertical scroll position as a 0.0..=1.0 ratio.
    pub(crate) ratio: f64,
    /// Index of the `id="h{n}"` anchor nearest the top, or `None`.
    pub(crate) current_heading_index: Option<usize>,
    /// Number of `id="h{n}"` headings in the rendered page.
    pub(crate) heading_count: usize,
}

/// A report about the page's viewport or a user action on it (`page` event),
/// tagged by `kind`.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(crate) enum PageEvent {
    /// The viewport moved or a document was rendered.
    ScrollState(ScrollState),
    /// The typed search was confirmed.
    SearchSubmit {
        /// What ended the typing.
        cause: SearchCause,
    },
    /// The typed search was abandoned.
    SearchEscape {
        /// What ended the typing.
        cause: SearchCause,
    },
    /// The close button of a confirmed search was clicked.
    SearchClose,
    /// An outline entry was clicked.
    OutlineJump {
        /// Index of the clicked `id="h{n}"` heading.
        index: usize,
    },
}

/// A scroll command payload (`scroll` emit).
#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct Scroll {
    /// Which way to scroll.
    pub(crate) action: ScrollAction,
}

/// A search-navigation payload (`searchNav` emit).
#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct SearchNav {
    /// Direction to move within the match set.
    pub(crate) dir: SearchDir,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn scroll_action_is_camel_case() {
        assert_eq!(
            serde_json::to_value(ScrollAction::HalfDown).unwrap(),
            json!("halfDown")
        );
        assert_eq!(
            serde_json::to_value(ScrollAction::Top).unwrap(),
            json!("top")
        );
    }

    #[test]
    fn content_renames_base_dir() {
        let c = Content {
            markdown: "# x".into(),
            base_dir: "/tmp".into(),
            scroll_to: ScrollTo::Preserve,
        };
        assert_eq!(
            serde_json::to_value(&c).unwrap(),
            json!({"markdown": "# x", "baseDir": "/tmp", "scrollTo": {"kind": "preserve"}})
        );
    }

    fn page_event(value: serde_json::Value) -> PageEvent {
        serde_json::from_value(value).expect("page event parses")
    }

    /// Asserts that a scroll-state report reads its camelCase fields, including a
    /// null heading index.
    ///
    /// Case: the page reports its viewport after rendering a document scrolled
    /// above its first heading.
    #[test]
    fn a_scroll_state_report_reads_camel_case() {
        assert_eq!(
            page_event(json!({
                "kind": "scrollState",
                "ratio": 0.5,
                "currentHeadingIndex": null,
                "headingCount": 4
            })),
            PageEvent::ScrollState(ScrollState {
                ratio: 0.5,
                current_heading_index: None,
                heading_count: 4,
            })
        );
    }

    /// Asserts that an outline jump reads its index.
    ///
    /// Case: the user clicks the third heading in the outline sidebar.
    #[test]
    fn an_outline_jump_reads_its_index() {
        assert_eq!(
            page_event(json!({"kind": "outlineJump", "index": 2})),
            PageEvent::OutlineJump { index: 2 }
        );
    }

    /// Asserts that the search-ending reports parse, reading their cause from camelCase strings.
    ///
    /// Case: the page reports Enter in the find box, a click on the body while typing, and a
    /// click on the close button of a confirmed search.
    #[test]
    fn search_reports_read_their_cause() {
        assert_eq!(
            page_event(json!({"kind": "searchSubmit", "cause": "key"})),
            PageEvent::SearchSubmit {
                cause: SearchCause::Key
            }
        );
        assert_eq!(
            page_event(json!({"kind": "searchEscape", "cause": "blur"})),
            PageEvent::SearchEscape {
                cause: SearchCause::Blur
            }
        );
        assert_eq!(
            page_event(json!({"kind": "searchClose"})),
            PageEvent::SearchClose
        );
    }

    /// Asserts that a relayed character serializes as a `text` field.
    ///
    /// Case: the user types `a` right after `/`, before the page holds focus.
    #[test]
    fn search_type_serializes_its_text() {
        assert_eq!(
            serde_json::to_value(SearchType { text: "a".into() }).expect("serializes"),
            json!({"text": "a"})
        );
    }

    #[test]
    fn scroll_to_serializes_tagged_camel_case() {
        assert_eq!(
            serde_json::to_value(ScrollTo::Preserve).unwrap(),
            json!({"kind": "preserve"})
        );
        assert_eq!(
            serde_json::to_value(ScrollTo::Top).unwrap(),
            json!({"kind": "top"})
        );
        assert_eq!(
            serde_json::to_value(ScrollTo::Ratio { ratio: 0.5 }).unwrap(),
            json!({"kind": "ratio", "ratio": 0.5})
        );
        assert_eq!(
            serde_json::to_value(ScrollTo::Slug {
                slug: "mounting".into()
            })
            .unwrap(),
            json!({"kind": "slug", "slug": "mounting"})
        );
    }

    #[test]
    fn content_includes_scroll_to() {
        let c = Content {
            markdown: "# x".into(),
            base_dir: "/tmp".into(),
            scroll_to: ScrollTo::Top,
        };
        assert_eq!(
            serde_json::to_value(&c).unwrap(),
            json!({"markdown": "# x", "baseDir": "/tmp", "scrollTo": {"kind": "top"}})
        );
    }

    #[test]
    fn navigate_request_reads_path_and_optional_fragment() {
        let a: NavigateRequest =
            serde_json::from_value(json!({"path": "a.md", "fragment": "sec"})).unwrap();
        assert_eq!(a.path, "a.md");
        assert_eq!(a.fragment.as_deref(), Some("sec"));
        let b: NavigateRequest =
            serde_json::from_value(json!({"path": "a.md", "fragment": null})).unwrap();
        assert_eq!(b.fragment, None);
    }

    #[test]
    fn stage_assets_request_reads_paths() {
        let r: StageAssetsRequest =
            serde_json::from_value(json!({"paths": ["a.png", "/b.png"]})).unwrap();
        assert_eq!(r.paths, vec!["a.png".to_string(), "/b.png".to_string()]);
    }

    #[test]
    fn stage_assets_response_serializes_urls_with_nulls() {
        let r = StageAssetsResponse {
            urls: vec![Some("_local/x.png".to_string()), None],
        };
        assert_eq!(
            serde_json::to_value(&r).unwrap(),
            json!({"urls": ["_local/x.png", null]})
        );
    }
}
