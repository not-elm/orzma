//! Wire types for the controller↔page protocol. Emit payloads serialize; call
//! params deserialize. All field names are camelCase on the wire.

use crate::document::Document;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Serialize};

/// A page request to navigate to a local Markdown file (`navigate` event).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NavigateRequest {
    /// The link's raw path (relative or absolute), percent-decoded by the page.
    pub(crate) path: String,
    /// The link's `#fragment`, if any, to scroll to after the document loads.
    #[serde(default)]
    pub(crate) fragment: Option<String>,
    /// The scroll position of the document the link was clicked in, as a
    /// 0.0..=1.0 ratio; 0.0 when the page leaves it out.
    #[serde(default)]
    pub(crate) ratio: f64,
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
    /// Whether this is another document the user moved to by a link or by
    /// going back; the page closes its search for it.
    pub(crate) navigated: bool,
}

impl Content {
    /// The content of `doc`, scrolled to `scroll_to` once rendered;
    /// `navigated` marks a document the user moved to.
    pub fn of_document(doc: &Document, scroll_to: ScrollTo, navigated: bool) -> Self {
        Self {
            markdown: doc.text.clone(),
            base_dir: doc.base_dir.display().to_string(),
            scroll_to,
            navigated,
        }
    }
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

/// A request from the page (`page` event), tagged by `kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(crate) enum PageEvent {
    /// The user asked to quit.
    Quit,
    /// The user asked to re-read the file.
    Reload,
    /// The user asked to go back to the previous document.
    Back,
}

/// A key the TUI received, handed to the page (`key` emit) under its DOM
/// `KeyboardEvent.key` name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct RelayedKey {
    /// The DOM `key` value, such as `j`, `G`, `" "`, `ArrowDown`, or `Escape`.
    pub(crate) key: String,
    /// Whether Control was held.
    pub(crate) ctrl: bool,
    /// Whether Alt (Option) was held.
    pub(crate) alt: bool,
    /// Whether Shift was held.
    pub(crate) shift: bool,
}

impl RelayedKey {
    /// The relayed form of `key`, or `None` for a key it gives no DOM name.
    /// A character keeps its case, so Shift+G is `G`; Shift+Tab is `Tab` with
    /// Shift held.
    pub fn from_key(key: KeyEvent) -> Option<Self> {
        let mut shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let name = match key.code {
            KeyCode::Char(c) => c.to_string(),
            KeyCode::Up => "ArrowUp".to_owned(),
            KeyCode::Down => "ArrowDown".to_owned(),
            KeyCode::Left => "ArrowLeft".to_owned(),
            KeyCode::Right => "ArrowRight".to_owned(),
            KeyCode::PageUp => "PageUp".to_owned(),
            KeyCode::PageDown => "PageDown".to_owned(),
            KeyCode::Home => "Home".to_owned(),
            KeyCode::End => "End".to_owned(),
            KeyCode::Enter => "Enter".to_owned(),
            KeyCode::Esc => "Escape".to_owned(),
            KeyCode::Tab => "Tab".to_owned(),
            KeyCode::BackTab => {
                shift = true;
                "Tab".to_owned()
            }
            KeyCode::Backspace => "Backspace".to_owned(),
            KeyCode::Delete => "Delete".to_owned(),
            _ => return None,
        };
        Some(Self {
            key: name,
            ctrl: key.modifiers.contains(KeyModifiers::CONTROL),
            alt: key.modifiers.contains(KeyModifiers::ALT),
            shift,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn relayed(code: KeyCode, modifiers: KeyModifiers) -> Option<RelayedKey> {
        RelayedKey::from_key(KeyEvent::new(code, modifiers))
    }

    fn named(key: &str, ctrl: bool, alt: bool, shift: bool) -> Option<RelayedKey> {
        Some(RelayedKey {
            key: key.to_owned(),
            ctrl,
            alt,
            shift,
        })
    }

    /// Asserts that each relayed key carries its DOM `KeyboardEvent.key` name
    /// and its modifiers.
    ///
    /// Case: the user presses keys in the TUI before the page holds focus,
    /// including Shift+G, Ctrl+D, Option+[, and Shift+Tab.
    #[test]
    fn relayed_keys_carry_dom_names() {
        let (none, shift) = (KeyModifiers::NONE, KeyModifiers::SHIFT);
        let modified = [
            (KeyCode::Char('j'), none, named("j", false, false, false)),
            (KeyCode::Char('G'), shift, named("G", false, false, true)),
            (KeyCode::Char(' '), none, named(" ", false, false, false)),
            (
                KeyCode::Char('d'),
                KeyModifiers::CONTROL,
                named("d", true, false, false),
            ),
            (
                KeyCode::Char('['),
                KeyModifiers::ALT,
                named("[", false, true, false),
            ),
            (KeyCode::BackTab, shift, named("Tab", false, false, true)),
            (KeyCode::BackTab, none, named("Tab", false, false, true)),
        ];
        for (code, modifiers, expected) in modified {
            assert_eq!(relayed(code, modifiers), expected, "{code:?} {modifiers:?}");
        }
        let unmodified = [
            (KeyCode::Up, "ArrowUp"),
            (KeyCode::Down, "ArrowDown"),
            (KeyCode::Left, "ArrowLeft"),
            (KeyCode::Right, "ArrowRight"),
            (KeyCode::PageUp, "PageUp"),
            (KeyCode::PageDown, "PageDown"),
            (KeyCode::Home, "Home"),
            (KeyCode::End, "End"),
            (KeyCode::Enter, "Enter"),
            (KeyCode::Esc, "Escape"),
            (KeyCode::Tab, "Tab"),
            (KeyCode::Backspace, "Backspace"),
            (KeyCode::Delete, "Delete"),
        ];
        for (code, name) in unmodified {
            assert_eq!(
                relayed(code, none),
                named(name, false, false, false),
                "{code:?}"
            );
        }
    }

    /// Asserts that keys without a DOM name here are not relayed.
    ///
    /// Case: the user presses F1 or Insert in the TUI before the page holds
    /// focus.
    #[test]
    fn keys_without_a_dom_name_are_not_relayed() {
        assert_eq!(relayed(KeyCode::F(1), KeyModifiers::NONE), None);
        assert_eq!(relayed(KeyCode::Insert, KeyModifiers::NONE), None);
    }

    /// Asserts that a relayed key serializes its name and its three modifiers.
    ///
    /// Case: the TUI hands Shift+G to the page as a `key` event.
    #[test]
    fn a_relayed_key_serializes_its_fields() {
        let key = RelayedKey::from_key(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT))
            .expect("a character has a DOM name");
        assert_eq!(
            serde_json::to_value(key).expect("serializes"),
            json!({"key": "G", "ctrl": false, "alt": false, "shift": true})
        );
    }

    /// Asserts that each page request reads its kind.
    ///
    /// Case: the user presses `q`, `r`, and Backspace in the page.
    #[test]
    fn page_requests_read_their_kind() {
        let parse = |value| serde_json::from_value::<PageEvent>(value).expect("page event parses");
        assert_eq!(parse(json!({"kind": "quit"})), PageEvent::Quit);
        assert_eq!(parse(json!({"kind": "reload"})), PageEvent::Reload);
        assert_eq!(parse(json!({"kind": "back"})), PageEvent::Back);
    }

    /// Asserts that a navigate request reads its scroll ratio, and takes 0.0
    /// when the page leaves it out.
    ///
    /// Case: the user clicks a link to another Markdown file halfway down the
    /// document.
    #[test]
    fn a_navigate_request_reads_its_ratio_or_takes_zero() {
        let with: NavigateRequest =
            serde_json::from_value(json!({"path": "a.md", "fragment": "sec", "ratio": 0.5}))
                .expect("parses");
        assert_eq!(with.path, "a.md");
        assert_eq!(with.fragment.as_deref(), Some("sec"));
        assert!((with.ratio - 0.5).abs() < f64::EPSILON);
        let without: NavigateRequest =
            serde_json::from_value(json!({"path": "a.md", "fragment": null})).expect("parses");
        assert_eq!(without.fragment, None);
        assert!(without.ratio.abs() < f64::EPSILON);
    }

    /// Asserts that content serializes camelCase keys, its scroll target, and
    /// whether the user moved to it.
    ///
    /// Case: the user follows a link to `b.md`, so the page receives it as a
    /// document moved to, scrolled to the top.
    #[test]
    fn content_serializes_camel_case_with_navigated() {
        let c = Content {
            markdown: "# x".into(),
            base_dir: "/tmp".into(),
            scroll_to: ScrollTo::Top,
            navigated: true,
        };
        assert_eq!(
            serde_json::to_value(&c).expect("serializes"),
            json!({"markdown": "# x", "baseDir": "/tmp", "scrollTo": {"kind": "top"}, "navigated": true})
        );
    }

    /// Asserts that each scroll target serializes as a camelCase `kind` tag
    /// with its fields.
    ///
    /// Case: the page receives content after a reload, a forward navigation,
    /// a back navigation, and a link to `file.md#mounting`.
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

    /// Asserts that a stage request reads its paths in order.
    ///
    /// Case: the page renders a document with a relative and an absolute image.
    #[test]
    fn stage_assets_request_reads_paths() {
        let r: StageAssetsRequest =
            serde_json::from_value(json!({"paths": ["a.png", "/b.png"]})).unwrap();
        assert_eq!(r.paths, vec!["a.png".to_string(), "/b.png".to_string()]);
    }

    /// Asserts that a stage response serializes a path it could not stage as
    /// `null`, in request order.
    ///
    /// Case: one of the two images in a document cannot be staged.
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
