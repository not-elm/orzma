//! Wire types between the chrome page and the controller. Field names are
//! camelCase on the wire.

use crate::address::{AddressTarget, SearchEngine};
use serde::{Deserialize, Serialize};
use url::Url;

/// The params of a `preview` or `submit` call.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct AddressRequest {
    /// The address input's text.
    pub(crate) text: String,
}

/// What Enter does with the address input's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum PreviewKind {
    /// Nothing; the input is blank.
    Empty,
    /// Opens an address.
    Open,
    /// Searches for the words.
    Search,
    /// Nothing; the text names an address that cannot be opened.
    Invalid,
}

/// The reply to a `preview` or `submit` call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Preview {
    /// What Enter does.
    pub(crate) kind: PreviewKind,
    /// The text the omnibox shows, such as `Open github.com`.
    pub(crate) label: String,
}

/// A report from the chrome page (`page` event), tagged by `kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(crate) enum PageEvent {
    /// Esc was pressed in the address bar.
    Cancel,
    /// The omnibox was clicked outside the address bar.
    OpenAddress,
}

impl Preview {
    /// What the address bar shows for `target`.
    pub fn of(target: &AddressTarget, engine: &SearchEngine) -> Self {
        let (kind, label) = match target {
            AddressTarget::Empty => (PreviewKind::Empty, String::new()),
            AddressTarget::Open(url) => (PreviewKind::Open, format!("Open {}", display_host(url))),
            AddressTarget::Search(_) => (PreviewKind::Search, format!("Search {}", engine.label())),
            AddressTarget::Invalid(reason) => (PreviewKind::Invalid, reason.to_string()),
        };
        Self { kind, label }
    }
}

/// The host of `url` with its port, or `url` itself when it does not parse.
fn display_host(url: &str) -> String {
    let Ok(parsed) = Url::parse(url) else {
        return url.to_owned();
    };
    match (parsed.host_str(), parsed.port()) {
        (Some(host), Some(port)) => format!("{host}:{port}"),
        (Some(host), None) => host.to_owned(),
        (None, _) => url.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::address::InvalidAddress;
    use serde_json::json;

    fn engine() -> SearchEngine {
        SearchEngine::from_template(None).expect("the default engine is valid")
    }

    /// Asserts what the address bar shows for each kind of target.
    ///
    /// Case: the user types nothing, a domain, a local server, a phrase, and a
    /// file URL.
    #[test]
    fn each_target_has_its_preview() {
        let engine = engine();
        let cases = [
            (AddressTarget::Empty, PreviewKind::Empty, ""),
            (
                AddressTarget::Open("https://github.com/".to_owned()),
                PreviewKind::Open,
                "Open github.com",
            ),
            (
                AddressTarget::Open("http://localhost:3000/".to_owned()),
                PreviewKind::Open,
                "Open localhost:3000",
            ),
            (
                AddressTarget::Search(engine.url_for("rust async")),
                PreviewKind::Search,
                "Search DuckDuckGo",
            ),
            (
                AddressTarget::Invalid(InvalidAddress::UnsupportedScheme("file".to_owned())),
                PreviewKind::Invalid,
                "Unsupported scheme: file",
            ),
        ];
        for (target, kind, label) in cases {
            assert_eq!(
                Preview::of(&target, &engine),
                Preview {
                    kind,
                    label: label.to_owned()
                },
                "{target:?}"
            );
        }
    }

    /// Asserts that a preview serializes with a camelCase `kind`.
    ///
    /// Case: the chrome page receives the reply to a `preview` call.
    #[test]
    fn a_preview_serializes_for_the_page() {
        let preview = Preview {
            kind: PreviewKind::Open,
            label: "Open github.com".to_owned(),
        };
        assert_eq!(
            serde_json::to_value(preview).expect("a preview serializes"),
            json!({ "kind": "open", "label": "Open github.com" })
        );
    }

    /// Asserts that the page's `cancel` and `openAddress` reports parse.
    ///
    /// Case: the user presses Esc in the address bar, then clicks the omnibox.
    #[test]
    fn page_events_parse_by_kind() {
        let parse = |value| serde_json::from_value::<PageEvent>(value).expect("a page event parses");
        assert_eq!(parse(json!({ "kind": "cancel" })), PageEvent::Cancel);
        assert_eq!(parse(json!({ "kind": "openAddress" })), PageEvent::OpenAddress);
    }

    /// Asserts that a `preview` or `submit` call's params parse.
    ///
    /// Case: the chrome page sends the address input's text.
    #[test]
    fn an_address_request_parses() {
        let request: AddressRequest =
            serde_json::from_value(json!({ "text": "rust" })).expect("a request parses");
        assert_eq!(request.text, "rust");
    }
}
