//! Classifies address-bar input as a URL to open or as words to search for.

use anyhow::{anyhow, bail};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use std::env::{self, VarError};
use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};
use url::Url;

/// The environment variable that overrides the search engine.
const SEARCH_URL_ENV: &str = "ORZBROWSER_SEARCH_URL";

/// The search engine used when [`SEARCH_URL_ENV`] is unset.
const DEFAULT_TEMPLATE: &str = "https://duckduckgo.com/?q={}";

/// The label of [`DEFAULT_TEMPLATE`].
const DEFAULT_LABEL: &str = "DuckDuckGo";

/// Bytes a search term escapes: everything except ASCII letters, digits, and
/// `-._~`.
const QUERY_ESCAPE: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// The search engine that words typed into the address bar go to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchEngine {
    template: String,
    label: String,
    home: String,
}

/// What the address bar does with its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AddressTarget {
    /// Nothing was typed.
    Empty,
    /// Open this normalized `http` or `https` URL.
    Open(String),
    /// Search at this URL.
    Search(String),
    /// The text names an address orzbrowser cannot open.
    Invalid(InvalidAddress),
}

/// Why typed text cannot be opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InvalidAddress {
    /// A scheme other than `http` and `https`, lowercased.
    UnsupportedScheme(String),
    /// An `http` or `https` URL that does not parse or has no host.
    Malformed,
}

impl SearchEngine {
    /// Reads the engine from `ORZBROWSER_SEARCH_URL`, or returns DuckDuckGo
    /// when the variable is unset or empty.
    ///
    /// # Errors
    /// Fails as [`SearchEngine::from_var`] does.
    pub fn from_env() -> anyhow::Result<Self> {
        Self::from_var(env::var(SEARCH_URL_ENV))
    }

    /// Builds the engine from the result of reading `ORZBROWSER_SEARCH_URL`:
    /// DuckDuckGo when the variable is unset or empty, and the template it
    /// holds otherwise.
    ///
    /// # Errors
    /// Fails when the value is not UTF-8, or is a template that
    /// [`SearchEngine::from_template`] rejects.
    pub fn from_var(value: Result<String, VarError>) -> anyhow::Result<Self> {
        match value {
            Ok(template) if !template.is_empty() => Self::from_template(Some(&template)),
            Ok(_) | Err(VarError::NotPresent) => Self::from_template(None),
            Err(VarError::NotUnicode(_)) => bail!("{SEARCH_URL_ENV} is not valid UTF-8"),
        }
    }

    /// Builds the engine from a URL template whose `{}` stands for the search
    /// term, or returns DuckDuckGo for `None`.
    ///
    /// The label is the template's host without a leading `www.`.
    ///
    /// # Errors
    /// Fails unless `template` holds exactly one `{}`, is an `http` or `https`
    /// URL, and keeps `{}` in its path, query, or fragment.
    pub fn from_template(template: Option<&str>) -> anyhow::Result<Self> {
        let Some(template) = template else {
            return Ok(Self {
                label: DEFAULT_LABEL.to_owned(),
                ..Self::from_template(Some(DEFAULT_TEMPLATE))?
            });
        };
        if template.matches("{}").count() != 1 {
            bail!("{SEARCH_URL_ENV} must contain exactly one {{}}: {template}");
        }
        let first = parse_web_url(&template.replace("{}", "a"), template)?;
        let second = parse_web_url(&template.replace("{}", "b"), template)?;
        if first.origin() != second.origin()
            || first.username() != second.username()
            || first.password() != second.password()
        {
            bail!("{SEARCH_URL_ENV} must keep {{}} in the path, query or fragment: {template}");
        }
        let host = first.host_str().unwrap_or_default();
        Ok(Self {
            template: template.to_owned(),
            label: host.strip_prefix("www.").unwrap_or(host).to_owned(),
            home: format!("{}/", first.origin().ascii_serialization()),
        })
    }

    /// The name the address bar shows for this engine.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The engine's front page: the template's origin followed by `/`.
    pub fn home(&self) -> &str {
        &self.home
    }

    /// The search URL for `query`, percent-encoded into the template's `{}`.
    pub fn url_for(&self, query: &str) -> String {
        let encoded = utf8_percent_encode(query, QUERY_ESCAPE).to_string();
        self.template.replacen("{}", &encoded, 1)
    }
}

impl AddressTarget {
    /// Classifies `input`, ignoring surrounding whitespace.
    ///
    /// A leading `?` searches for the rest. An explicit `http` or `https` URL
    /// opens as typed, and any other scheme is invalid. Text with whitespace
    /// is searched for. A localhost, IP, or one-label-with-port host opens
    /// over `http`, and a dotted name whose last label has two or more
    /// letters opens over `https`. Everything else is searched for.
    pub fn parse(input: &str, engine: &SearchEngine) -> Self {
        let input = input.trim();
        if input.is_empty() {
            return Self::Empty;
        }
        if let Some(rest) = input.strip_prefix('?') {
            let query = rest.trim();
            return if query.is_empty() {
                Self::Empty
            } else {
                Self::Search(engine.url_for(query))
            };
        }
        if let Some(scheme) = explicit_scheme(input) {
            return Self::with_scheme(input, &scheme, engine);
        }
        if input.contains(char::is_whitespace) {
            return Self::Search(engine.url_for(input));
        }
        match schemeless_url(input) {
            Some(url) => Self::Open(url),
            None => Self::Search(engine.url_for(input)),
        }
    }

    fn with_scheme(input: &str, scheme: &str, engine: &SearchEngine) -> Self {
        if !matches!(scheme, "http" | "https") {
            return Self::Invalid(InvalidAddress::UnsupportedScheme(scheme.to_owned()));
        }
        match Url::parse(input) {
            Ok(url) if url.host_str().is_some_and(|host| !host.is_empty()) => {
                Self::Open(url.into())
            }
            _ if input.contains(char::is_whitespace) => Self::Search(engine.url_for(input)),
            _ => Self::Invalid(InvalidAddress::Malformed),
        }
    }
}

impl fmt::Display for InvalidAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedScheme(scheme) => write!(f, "Unsupported scheme: {scheme}"),
            Self::Malformed => f.write_str("Not a valid URL"),
        }
    }
}

fn parse_web_url(candidate: &str, template: &str) -> anyhow::Result<Url> {
    let url = Url::parse(candidate)
        .map_err(|e| anyhow!("{SEARCH_URL_ENV} is not a URL ({e}): {template}"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none_or(str::is_empty) {
        bail!("{SEARCH_URL_ENV} must be an http or https URL: {template}");
    }
    Ok(url)
}

/// The lowercased scheme of `input` when it starts with `scheme://`.
fn explicit_scheme(input: &str) -> Option<String> {
    let (scheme, _) = input.split_once("://")?;
    let mut chars = scheme.chars();
    let valid = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'));
    valid.then(|| scheme.to_ascii_lowercase())
}

/// The URL `input` names when it is a host, optionally with a port and a
/// path, query, or fragment.
fn schemeless_url(input: &str) -> Option<String> {
    let authority_end = input.find(['/', '?', '#']).unwrap_or(input.len());
    let authority = &input[..authority_end];
    if authority.contains('@') {
        return None;
    }
    let (host, has_port) = split_port(authority);
    let scheme = scheme_for(host, has_port)?;
    let url = Url::parse(&format!("{scheme}://{input}")).ok()?;
    url.host_str()
        .is_some_and(|host| !host.is_empty())
        .then(|| url.into())
}

/// Splits a trailing `:digits` port off `authority`; in a bracketed IPv6
/// authority, only a port after the `]` counts.
fn split_port(authority: &str) -> (&str, bool) {
    let search_from = if authority.starts_with('[') {
        match authority.find(']') {
            Some(end) => end + 1,
            None => return (authority, false),
        }
    } else {
        0
    };
    let Some(offset) = authority[search_from..].rfind(':') else {
        return (authority, false);
    };
    let colon = search_from + offset;
    let port = &authority[colon + 1..];
    if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) {
        (&authority[..colon], true)
    } else {
        (authority, false)
    }
}

/// The scheme to put in front of `host`, or `None` when `host` does not look
/// like a host.
fn scheme_for(host: &str, has_port: bool) -> Option<&'static str> {
    let lower = host.to_ascii_lowercase();
    let local = lower == "localhost" || lower.ends_with(".localhost");
    let ip = host.parse::<Ipv4Addr>().is_ok()
        || host
            .strip_prefix('[')
            .and_then(|inner| inner.strip_suffix(']'))
            .is_some_and(|inner| inner.parse::<Ipv6Addr>().is_ok());
    if local || ip {
        Some("http")
    } else if is_domain(host) {
        Some("https")
    } else if has_port && is_label(host) {
        Some("http")
    } else {
        None
    }
}

/// Whether `host` is two or more labels whose last one is two or more
/// letters; one trailing dot is allowed.
fn is_domain(host: &str) -> bool {
    let host = host.strip_suffix('.').unwrap_or(host);
    let labels: Vec<&str> = host.split('.').collect();
    labels.len() >= 2
        && labels.iter().all(|label| is_label(label))
        && labels
            .last()
            .is_some_and(|tld| tld.chars().count() >= 2 && tld.chars().all(char::is_alphabetic))
}

fn is_label(label: &str) -> bool {
    !label.is_empty() && label.chars().all(|c| c.is_alphanumeric() || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn engine() -> SearchEngine {
        SearchEngine::from_template(None).expect("the default engine is valid")
    }

    fn parse(input: &str) -> AddressTarget {
        AddressTarget::parse(input, &engine())
    }

    fn open(url: &str) -> AddressTarget {
        AddressTarget::Open(url.to_owned())
    }

    fn search(query: &str) -> AddressTarget {
        AddressTarget::Search(engine().url_for(query))
    }

    /// Asserts that a domain opens over `https`, and that an explicit http(s)
    /// URL opens as typed.
    ///
    /// Case: the user types a domain, a domain with a path, a URL copied with
    /// an uppercase scheme, a name with a trailing dot, a domain with its
    /// default port, or a full `http` URL.
    #[test]
    fn a_domain_opens_over_https() {
        for (input, url) in [
            ("github.com", "https://github.com/"),
            ("docs.rs/serde", "https://docs.rs/serde"),
            ("HTTPS://Example.com", "https://example.com/"),
            ("example.com.", "https://example.com./"),
            ("example.com:443", "https://example.com/"),
            ("http://example.com/a", "http://example.com/a"),
        ] {
            assert_eq!(parse(input), open(url), "{input}");
        }
    }

    /// Asserts that local hosts, IP addresses, and a one-label host with a
    /// port open over `http`.
    ///
    /// Case: a developer opens a dev server on localhost, a router by its IP,
    /// or a LAN machine by its name and port.
    #[test]
    fn a_local_host_opens_over_http() {
        for (input, url) in [
            ("localhost", "http://localhost/"),
            ("localhost:3000", "http://localhost:3000/"),
            ("app.localhost:3000", "http://app.localhost:3000/"),
            ("192.168.0.1:8080", "http://192.168.0.1:8080/"),
            ("[::1]:8080", "http://[::1]:8080/"),
            ("devbox:8080", "http://devbox:8080/"),
        ] {
            assert_eq!(parse(input), open(url), "{input}");
        }
    }

    /// Asserts that words, and text that only resembles an address, are
    /// searched for.
    ///
    /// Case: the user types a word, a phrase, a version number, a shortened
    /// IP, an abbreviation, an email address, a `mailto:` link, a sentence
    /// that starts with a URL, or a Japanese query.
    #[test]
    fn text_that_is_not_an_address_is_searched() {
        for input in [
            "rust",
            "rust async",
            "3.14",
            "127.1",
            "e.g.",
            "user@example.com",
            "mailto:a@example.com",
            "http://example.com is down",
            "東京 天気",
        ] {
            assert_eq!(parse(input), search(input), "{input}");
        }
    }

    /// Asserts that a leading `?` searches for the rest even when the rest
    /// looks like a domain.
    ///
    /// Case: the user wants search results for a file name such as `node.js`.
    #[test]
    fn a_leading_question_mark_forces_a_search() {
        assert_eq!(parse("? github.com"), search("github.com"));
        assert_eq!(parse("?node.js"), search("node.js"));
    }

    /// Asserts that blank input and a bare `?` are empty.
    ///
    /// Case: the user clears the address bar, or types only `?`, and presses
    /// Enter.
    #[test]
    fn blank_input_is_empty() {
        for input in ["", "   ", "?", "?  "] {
            assert_eq!(parse(input), AddressTarget::Empty, "{input:?}");
        }
    }

    /// Asserts that a scheme other than http and https is rejected with its
    /// lowercased name, and that an http(s) URL without a host is malformed.
    ///
    /// Case: the user types a local file URL, an `about:` page, an FTP URL,
    /// or only `https://`.
    #[test]
    fn other_schemes_and_hostless_urls_are_invalid() {
        for (input, scheme) in [
            ("file:///etc", "file"),
            ("about://blank", "about"),
            ("FTP://example.com", "ftp"),
        ] {
            assert_eq!(
                parse(input),
                AddressTarget::Invalid(InvalidAddress::UnsupportedScheme(scheme.to_owned())),
                "{input}"
            );
        }
        assert_eq!(
            parse("https://"),
            AddressTarget::Invalid(InvalidAddress::Malformed)
        );
        assert_eq!(
            InvalidAddress::UnsupportedScheme("file".to_owned()).to_string(),
            "Unsupported scheme: file"
        );
    }

    /// Asserts that surrounding whitespace is ignored and a space in an
    /// explicit URL's path is percent-encoded.
    ///
    /// Case: the user pastes a URL with a trailing newline, or one whose path
    /// contains a space.
    #[test]
    fn pasted_urls_open_trimmed_and_encoded() {
        assert_eq!(
            parse("  https://example.com  \n"),
            open("https://example.com/")
        );
        assert_eq!(
            parse("https://example.com/a b"),
            open("https://example.com/a%20b")
        );
    }

    /// Asserts that a search term is percent-encoded, with spaces as `%20`,
    /// and that braces in the term do not reach the template.
    ///
    /// Case: the user searches for a phrase with `&`, `+`, `%`, `{}`, and
    /// Japanese, and for a term made only of unreserved characters.
    #[test]
    fn search_terms_are_percent_encoded() {
        let engine = engine();
        assert_eq!(
            engine.url_for("rust & c++"),
            "https://duckduckgo.com/?q=rust%20%26%20c%2B%2B"
        );
        assert_eq!(
            engine.url_for("100% {}"),
            "https://duckduckgo.com/?q=100%25%20%7B%7D"
        );
        assert_eq!(
            engine.url_for("東京"),
            "https://duckduckgo.com/?q=%E6%9D%B1%E4%BA%AC"
        );
        assert_eq!(
            engine.url_for("a-b.c_d~e"),
            "https://duckduckgo.com/?q=a-b.c_d~e"
        );
    }

    /// Asserts that the default engine is DuckDuckGo with its front page as
    /// home.
    ///
    /// Case: the user has not set `ORZBROWSER_SEARCH_URL`.
    #[test]
    fn the_default_engine_is_duckduckgo() {
        let engine = engine();
        assert_eq!(engine.label(), "DuckDuckGo");
        assert_eq!(engine.home(), "https://duckduckgo.com/");
    }

    /// Asserts that an unset or empty variable selects DuckDuckGo, and that a
    /// value that is not UTF-8 is an error.
    ///
    /// Case: the user leaves `ORZBROWSER_SEARCH_URL` unset, exports it empty,
    /// or sets it to bytes that are not UTF-8.
    #[test]
    fn the_variable_falls_back_only_when_unset_or_empty() {
        let unset = SearchEngine::from_var(Err(VarError::NotPresent))
            .expect("an unset variable selects the default");
        assert_eq!(unset.label(), "DuckDuckGo");
        let empty = SearchEngine::from_var(Ok(String::new()))
            .expect("an empty variable selects the default");
        assert_eq!(empty.label(), "DuckDuckGo");
        let custom = SearchEngine::from_var(Ok("https://www.google.com/search?q={}".to_owned()))
            .expect("a Google template is valid");
        assert_eq!(custom.label(), "google.com");
        let not_utf8 = VarError::NotUnicode(OsString::from("x"));
        assert!(SearchEngine::from_var(Err(not_utf8)).is_err());
    }

    /// Asserts that a custom template sets the label from its host and a home
    /// that keeps its port.
    ///
    /// Case: the user points `ORZBROWSER_SEARCH_URL` at Google, at a
    /// self-hosted engine on a port, or at an engine that takes the term in
    /// its path.
    #[test]
    fn a_custom_template_sets_the_label_and_home() {
        let google = SearchEngine::from_template(Some("https://www.google.com/search?q={}"))
            .expect("a Google template is valid");
        assert_eq!(google.label(), "google.com");
        assert_eq!(google.home(), "https://www.google.com/");
        assert_eq!(
            google.url_for("rust"),
            "https://www.google.com/search?q=rust"
        );

        let local = SearchEngine::from_template(Some("http://localhost:8888/search?q={}"))
            .expect("a local template is valid");
        assert_eq!(local.label(), "localhost");
        assert_eq!(local.home(), "http://localhost:8888/");

        let in_path = SearchEngine::from_template(Some("https://example.com/search/{}"))
            .expect("a path template is valid");
        assert_eq!(in_path.url_for("a b"), "https://example.com/search/a%20b");
    }

    /// Asserts that a template is rejected unless it holds exactly one `{}`
    /// in its path, query, or fragment, and is an http(s) URL.
    ///
    /// Case: the user sets `ORZBROWSER_SEARCH_URL` to a URL without `{}`, with
    /// two, with `{}` in the scheme, host, port, or user name, to an FTP URL,
    /// or to text.
    #[test]
    fn a_bad_template_is_rejected() {
        for bad in [
            "https://duckduckgo.com/",
            "https://x.com/?q={}&r={}",
            "ht{}tps://example.com/?q=x",
            "https://{}.example.com/",
            "https://example.com:{}/",
            "https://{}@example.com/?q=x",
            "ftp://example.com/{}",
            "not a url {}",
        ] {
            assert!(SearchEngine::from_template(Some(bad)).is_err(), "{bad}");
        }
    }
}
