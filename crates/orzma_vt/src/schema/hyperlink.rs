//! Vocabulary for OSC 8 hyperlinks.

/// OSC 8 hyperlink: an interned id → URI mapping.
///
/// Cells reference these via `Run::hyperlink_id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hyperlink {
    /// This hyperlink's id.
    pub id: HyperlinkId,
    /// The hyperlink target URI.
    pub uri: HyperlinkUri,
}
/// Monotonic hyperlink id.
///
/// # Invariants
///
/// Callers outside the interner MUST NOT construct `HyperlinkId(0)`;
/// it is the universal "no hyperlink" sentinel the renderer's
/// `hyperlink_id != 0u` branch depends on.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct HyperlinkId(pub u32);

/// OSC 8 hyperlink target URI.
#[derive(Clone, Eq, PartialEq, Hash, Debug)]
pub struct HyperlinkUri(String);

impl HyperlinkUri {
    /// Wraps a string as a hyperlink URI.
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Returns the underlying string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

const ALLOWED_SCHEMES: &[&str] = &["http", "https", "mailto", "ftp"];

/// Returns `true` when `uri` carries a scheme on the v1 allowlist
/// (`http`, `https`, `mailto`, `ftp`), case-insensitive.
pub fn is_allowed(uri: &str) -> bool {
    scheme_of(uri)
        .map(|s| s.to_ascii_lowercase())
        .is_some_and(|s| ALLOWED_SCHEMES.contains(&s.as_str()))
}

/// Parses an RFC 3986 scheme: first byte ALPHA, continuation
/// ALPHA / DIGIT / `+` / `-` / `.`. Returns `None` for malformed input.
fn scheme_of(uri: &str) -> Option<&str> {
    let (scheme, _) = uri.split_once(':')?;
    let mut bytes = scheme.bytes();
    let first = bytes.next()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    if !bytes.all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'-' || b == b'.') {
        return None;
    }
    Some(scheme)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_allowed_accepts_canonical_schemes_case_insensitive() {
        assert!(is_allowed("http://example.com"));
        assert!(is_allowed("HTTPS://example.com"));
        assert!(is_allowed("Mailto:foo@example"));
        assert!(is_allowed("ftp://example.com"));
    }

    #[test]
    fn is_allowed_rejects_dangerous_or_unknown_schemes() {
        assert!(!is_allowed("javascript:alert(1)"));
        assert!(!is_allowed("file:///etc/passwd"));
        assert!(!is_allowed("data:text/html,<script>"));
        assert!(!is_allowed("vscode://"));
        assert!(!is_allowed("vscode://example.com"));
        assert!(!is_allowed(""));
        assert!(!is_allowed("no-colon-here"));
    }
}
