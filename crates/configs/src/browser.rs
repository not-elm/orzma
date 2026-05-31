//! Browser activity configuration: search-engine template for the toolbar
//! URL bar's Chrome-style omnibox behavior, plus the omnibox classifier
//! used by both the toolbar (via the TS port) and the `ozmux browser` CLI.

use serde::{Deserialize, Serialize};

const DEFAULT_SEARCH_TEMPLATE: &str = "https://duckduckgo.com/?q={query}";

const QUERY_PLACEHOLDER: &str = "{query}";

const KNOWN_SCHEME_PREFIXES: &[&str] = &["about:", "data:", "chrome:", "file:", "view-source:"];

/// Fully-resolved browser configuration.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BrowserConfig {
    /// Search-engine URL template. The literal `{query}` placeholder is
    /// substituted with the URL-encoded query at navigate time.
    pub search_template: String,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            search_template: DEFAULT_SEARCH_TEMPLATE.into(),
        }
    }
}

/// Per-field-optional view of `[browser]` for TOML deserialization.
#[derive(Deserialize, Default, Clone, Debug)]
pub(crate) struct BrowserPatch {
    /// Optional `[browser].search_template` override.
    pub search_template: Option<String>,
}

impl BrowserPatch {
    /// Applies any populated fields onto `base` and returns the merged result.
    pub fn apply_to(self, base: BrowserConfig) -> BrowserConfig {
        BrowserConfig {
            search_template: self.search_template.unwrap_or(base.search_template),
        }
    }
}

/// Resolves a user-typed input into a fully-qualified URL using the
/// omnibox algorithm.
///
/// Mirrors `daemon/frontend/src/browser/omnibox.ts::resolveOmniboxInput`.
/// Inputs that look like URLs are returned ready for navigation
/// (`https://` is prepended for bare hosts); everything else is rendered
/// into `search_template` after percent-encoding (compatible with the
/// frontend's `encodeURIComponent`). Empty input maps to an empty string
/// so callers can treat it as "no initial URL".
pub fn resolve_omnibox_input(raw: &str, search_template: &str) -> String {
    let input = raw.trim();
    if input.is_empty() {
        return String::new();
    }

    if has_scheme_prefix(input) {
        return input.to_string();
    }
    if KNOWN_SCHEME_PREFIXES.iter().any(|p| input.starts_with(p)) {
        return input.to_string();
    }

    if let Some(rest) = input.strip_prefix('?') {
        return search_for(rest.trim_start(), search_template);
    }

    let first_structural = input.find(['.', ':', '?']);
    let first_space = input.find(|c: char| c.is_whitespace() || c == '"');
    if let Some(s) = first_space
        && first_structural.is_none_or(|t| s < t)
    {
        return search_for(input, search_template);
    }

    let host_part = input.split(['/', '?', '#']).next().unwrap_or("");
    if looks_like_host(host_part) {
        return format!("https://{input}");
    }

    search_for(input, search_template)
}

fn has_scheme_prefix(input: &str) -> bool {
    let bytes = input.as_bytes();
    let mut i = 0;
    if bytes.is_empty() || !bytes[0].is_ascii_alphabetic() {
        return false;
    }
    i += 1;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.') {
            i += 1;
        } else {
            break;
        }
    }
    bytes.get(i..i + 3) == Some(b"://")
}

fn search_for(query: &str, template: &str) -> String {
    template.replace(QUERY_PLACEHOLDER, &encode_uri_component(query))
}

/// Percent-encodes `s` to match JavaScript's `encodeURIComponent`: every
/// byte outside the unreserved set `A-Za-z0-9 - _ . ! ~ * ' ( )` is
/// rendered as `%XX` over the UTF-8 bytes.
fn encode_uri_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        let unreserved = b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')'
            );
        if unreserved {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(hex_upper(b >> 4));
            out.push(hex_upper(b & 0x0f));
        }
    }
    out
}

fn hex_upper(n: u8) -> char {
    match n {
        0..=9 => (b'0' + n) as char,
        10..=15 => (b'A' + (n - 10)) as char,
        _ => unreachable!(),
    }
}

fn looks_like_host(host: &str) -> bool {
    if host.is_empty() {
        return false;
    }

    let mut parts = host.splitn(2, ':');
    let hostname = parts.next().unwrap_or("");
    if let Some(port) = parts.next()
        && (port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()))
    {
        return false;
    }

    if hostname == "localhost" {
        return true;
    }
    if is_ipv4_literal(hostname) {
        return true;
    }
    if !hostname.contains('.') {
        return false;
    }

    let labels: Vec<&str> = hostname.split('.').collect();
    if !labels.iter().all(|l| is_domain_label(l)) {
        return false;
    }
    let tld = labels.last().copied().unwrap_or("");
    tld.len() >= 2 && tld.bytes().all(|b| b.is_ascii_alphabetic())
}

fn is_ipv4_literal(s: &str) -> bool {
    let octets: Vec<&str> = s.split('.').collect();
    if octets.len() != 4 {
        return false;
    }
    octets.iter().all(|o| {
        let len = o.len();
        (1..=3).contains(&len) && o.bytes().all(|b| b.is_ascii_digit())
    })
}

fn is_domain_label(label: &str) -> bool {
    if label.is_empty() {
        return false;
    }
    let bytes = label.as_bytes();
    if !bytes[0].is_ascii_alphanumeric() || !bytes[bytes.len() - 1].is_ascii_alphanumeric() {
        return false;
    }
    bytes
        .iter()
        .all(|&b| b.is_ascii_alphanumeric() || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_uses_duckduckgo_template() {
        let c = BrowserConfig::default();
        assert_eq!(c.search_template, "https://duckduckgo.com/?q={query}");
    }

    #[test]
    fn empty_patch_returns_base() {
        let merged = BrowserPatch::default().apply_to(BrowserConfig::default());
        assert_eq!(merged, BrowserConfig::default());
    }

    #[test]
    fn template_override_applies() {
        let patch = BrowserPatch {
            search_template: Some("https://www.google.com/search?q={query}".into()),
        };
        let merged = patch.apply_to(BrowserConfig::default());
        assert_eq!(
            merged.search_template,
            "https://www.google.com/search?q={query}"
        );
    }

    const DDG: &str = DEFAULT_SEARCH_TEMPLATE;
    const CUSTOM: &str = "https://example.com/?q={query}";

    fn ddg_search(q: &str) -> String {
        format!("https://duckduckgo.com/?q={}", encode_uri_component(q))
    }

    #[test]
    fn keeps_fully_qualified_https_urls_intact() {
        assert_eq!(
            resolve_omnibox_input("https://example.com/path?a=1", DDG),
            "https://example.com/path?a=1"
        );
    }

    #[test]
    fn keeps_other_registered_schemes_intact() {
        assert_eq!(
            resolve_omnibox_input("http://example.com", DDG),
            "http://example.com"
        );
        assert_eq!(
            resolve_omnibox_input("ftp://example.com", DDG),
            "ftp://example.com"
        );
    }

    #[test]
    fn keeps_known_scheme_prefixes_intact() {
        for input in [
            "about:blank",
            "data:text/html,hi",
            "chrome://flags",
            "file:///tmp/a",
            "view-source:https://x.com",
        ] {
            assert_eq!(resolve_omnibox_input(input, DDG), input);
        }
    }

    #[test]
    fn prepends_https_for_dotted_domains() {
        assert_eq!(
            resolve_omnibox_input("example.com", DDG),
            "https://example.com"
        );
        assert_eq!(
            resolve_omnibox_input("example.com/path", DDG),
            "https://example.com/path"
        );
    }

    #[test]
    fn accepts_localhost_as_host() {
        assert_eq!(resolve_omnibox_input("localhost", DDG), "https://localhost");
        assert_eq!(
            resolve_omnibox_input("localhost:3000", DDG),
            "https://localhost:3000"
        );
        assert_eq!(
            resolve_omnibox_input("localhost:3000/api", DDG),
            "https://localhost:3000/api"
        );
    }

    #[test]
    fn accepts_ipv4_literals() {
        assert_eq!(resolve_omnibox_input("127.0.0.1", DDG), "https://127.0.0.1");
        assert_eq!(
            resolve_omnibox_input("127.0.0.1:8080/x", DDG),
            "https://127.0.0.1:8080/x"
        );
    }

    #[test]
    fn accepts_host_port_combos() {
        assert_eq!(
            resolve_omnibox_input("example.com:8080", DDG),
            "https://example.com:8080"
        );
    }

    #[test]
    fn rejects_single_bare_word_as_host() {
        assert_eq!(resolve_omnibox_input("hello", DDG), ddg_search("hello"));
    }

    #[test]
    fn rejects_numeric_tld() {
        assert_eq!(resolve_omnibox_input("foo.123", DDG), ddg_search("foo.123"));
    }

    #[test]
    fn routes_whitespace_input_to_search() {
        assert_eq!(
            resolve_omnibox_input("hello world", DDG),
            ddg_search("hello world")
        );
    }

    #[test]
    fn routes_claude_code_to_search() {
        assert_eq!(
            resolve_omnibox_input("claude code", DDG),
            ddg_search("claude code")
        );
    }

    #[test]
    fn leading_question_mark_forces_search() {
        assert_eq!(
            resolve_omnibox_input("?example.com", DDG),
            ddg_search("example.com")
        );
    }

    #[test]
    fn quoted_phrases_route_to_search() {
        assert_eq!(
            resolve_omnibox_input("\"exact phrase\"", DDG),
            ddg_search("\"exact phrase\"")
        );
    }

    #[test]
    fn space_before_dot_routes_to_search() {
        assert_eq!(
            resolve_omnibox_input("foo bar.com", DDG),
            ddg_search("foo bar.com")
        );
    }

    #[test]
    fn custom_template_is_used_when_provided() {
        assert_eq!(
            resolve_omnibox_input("hello world", CUSTOM),
            format!(
                "https://example.com/?q={}",
                encode_uri_component("hello world")
            )
        );
    }

    #[test]
    fn empty_input_returns_empty() {
        assert_eq!(resolve_omnibox_input("", DDG), "");
        assert_eq!(resolve_omnibox_input("   ", DDG), "");
    }

    #[test]
    fn trims_before_deciding() {
        assert_eq!(
            resolve_omnibox_input("  example.com  ", DDG),
            "https://example.com"
        );
    }

    #[test]
    fn encodes_special_characters_in_search_queries() {
        assert_eq!(resolve_omnibox_input("a&b=c", DDG), ddg_search("a&b=c"));
    }

    #[test]
    fn default_template_constant_contains_placeholder() {
        assert!(DEFAULT_SEARCH_TEMPLATE.contains("{query}"));
        assert!(DEFAULT_SEARCH_TEMPLATE.starts_with("https://"));
    }

    #[test]
    fn encode_uri_component_matches_javascript_unreserved_set() {
        assert_eq!(
            encode_uri_component("abcXYZ0123-_.!~*'()"),
            "abcXYZ0123-_.!~*'()"
        );
        assert_eq!(encode_uri_component(" "), "%20");
        assert_eq!(encode_uri_component("&"), "%26");
        assert_eq!(encode_uri_component("="), "%3D");
        assert_eq!(encode_uri_component("/"), "%2F");
        assert_eq!(encode_uri_component("日本"), "%E6%97%A5%E6%9C%AC");
    }
}
