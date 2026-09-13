//! OSC 8 hyperlink vocabulary and the id interner that dedupes it.

use std::collections::HashMap;
use std::num::NonZeroU32;

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
/// The zero value stands for "no hyperlink" and is not representable.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct HyperlinkId(NonZeroU32);

impl HyperlinkId {
    /// The id `value` names; `None` when `value` is zero.
    pub const fn new(value: u32) -> Option<Self> {
        match NonZeroU32::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    /// The id as a plain integer, which is never zero.
    pub const fn get(self) -> u32 {
        self.0.get()
    }
}

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

/// Returns `true` when `uri` carries a scheme on the allowlist
/// (`http`, `https`, `mailto`, `ftp`), case-insensitive.
pub fn is_allowed(uri: &str) -> bool {
    scheme_of(uri)
        .map(|s| s.to_ascii_lowercase())
        .is_some_and(|s| ALLOWED_SCHEMES.contains(&s.as_str()))
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct HyperlinkSourceId(String);

impl HyperlinkSourceId {
    pub(crate) fn new(id: String) -> Self {
        Self(id)
    }
}

#[derive(Debug, Eq, PartialEq, Hash)]
pub(crate) struct SourceHyperlink {
    pub id: HyperlinkSourceId,
    pub uri: HyperlinkUri,
}

/// Maps each `(source id, uri)` pair to a single [`HyperlinkId`],
/// minting a fresh id the first time a pair is seen and returning the id
/// already on file on repeats.
// TODO: release the entries of links no cell references any more.
// Growth tracks OSC 8 sequences received rather than links visible on
// screen, since opening a link mints unconditionally and a link never
// printed still grows these maps. An id-bearing open grows both maps
// and stores the target twice, once cloned into `id_to_uri` and once
// inside the `SourceHyperlink` key of `source_to_id`. Nothing reclaims
// an entry, not even a full reset, so recovery requires killing the
// pane.
pub(crate) struct HyperlinkInterner {
    next: NonZeroU32,
    id_to_uri: HashMap<HyperlinkId, HyperlinkUri>,
    source_to_id: HashMap<SourceHyperlink, HyperlinkId>,
}

impl HyperlinkInterner {
    /// Constructs an empty interner.
    ///
    /// The first id handed out is one.
    pub(crate) fn new() -> Self {
        Self {
            next: NonZeroU32::MIN,
            id_to_uri: HashMap::new(),
            source_to_id: HashMap::new(),
        }
    }

    pub(crate) fn intern(&mut self, source: SourceHyperlink) -> HyperlinkId {
        if let Some(id) = self.source_to_id.get(&source) {
            return *id;
        }
        let id = self.mint(source.uri.clone());
        self.source_to_id.insert(source, id);
        id
    }

    /// The id an `OSC 8` opens for `uri` under `id`.
    ///
    /// A nonempty `id` is a lookup key: a later open naming the same id
    /// and uri returns the id already on file. An absent or empty `id`
    /// returns a fresh id on every call, so two such links never join.
    pub(crate) fn open(&mut self, id: Option<String>, uri: HyperlinkUri) -> HyperlinkId {
        match id.filter(|id| !id.is_empty()) {
            Some(id) => self.intern(SourceHyperlink {
                id: HyperlinkSourceId::new(id),
                uri,
            }),
            None => self.mint(uri),
        }
    }

    #[inline]
    pub(crate) fn extract(&self, id: &HyperlinkId) -> Option<&HyperlinkUri> {
        self.id_to_uri.get(id)
    }

    /// Hands out a fresh id for `uri` without recording a lookup key.
    ///
    /// Ids saturate rather than wrap, so once `u32::MAX` ids have been
    /// handed out every later call returns that same id.
    fn mint(&mut self, uri: HyperlinkUri) -> HyperlinkId {
        let id = HyperlinkId(self.next);
        self.next = self.next.saturating_add(1);
        self.id_to_uri.insert(id, uri);
        id
    }
}

impl Default for HyperlinkInterner {
    fn default() -> Self {
        Self::new()
    }
}

const ALLOWED_SCHEMES: &[&str] = &["http", "https", "mailto", "ftp"];

/// Parses an RFC 3986 scheme. The first byte is ALPHA, and each later
/// byte is ALPHA, DIGIT, `+`, `-`, or `.`. Returns `None` for malformed
/// input.
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
    use std::collections::HashSet;

    fn source(id: &str, uri: &str) -> SourceHyperlink {
        SourceHyperlink {
            id: HyperlinkSourceId::new(id.to_owned()),
            uri: HyperlinkUri::new(uri.to_owned()),
        }
    }

    fn id(value: u32) -> HyperlinkId {
        HyperlinkId::new(value).expect("nonzero")
    }

    /// Asserts that interning an equal key twice returns the same id.
    ///
    /// Case: one OSC 8 link spans many cells of a row, and the frame
    /// builder builds a fresh key for every cell it walks.
    #[test]
    fn repeated_key_returns_the_same_id() {
        let mut interner = HyperlinkInterner::new();
        let first = interner.intern(source("1", "https://a.example"));
        let second = interner.intern(source("1", "https://a.example"));
        assert_eq!(first, second);
    }

    /// Asserts that one uri under two source ids yields distinct ids.
    ///
    /// Case: a program prints two OSC 8 links to the same URL without an
    /// explicit `id=`, so alacritty auto-numbers them `0_alacritty` and
    /// `1_alacritty`.
    #[test]
    fn auto_generated_source_ids_keep_identical_uris_distinct() {
        let mut interner = HyperlinkInterner::new();
        let first = interner.intern(source("0_alacritty", "https://a.example"));
        let second = interner.intern(source("1_alacritty", "https://a.example"));
        assert_ne!(first, second);
    }

    /// Asserts that one source id reused across two uris yields distinct ids.
    ///
    /// Case: an application reuses `id=1` for a second, unrelated URL.
    #[test]
    fn reused_source_id_with_different_uri_yields_distinct_ids() {
        let mut interner = HyperlinkInterner::new();
        let first = interner.intern(source("1", "https://a.example"));
        let second = interner.intern(source("1", "https://b.example"));
        assert_ne!(first, second);
    }

    /// Asserts that interning one key from separate rows keeps a single id.
    ///
    /// Case: a long URL wraps at the terminal edge, so the frame builder
    /// coalesces each row independently and interns the same link once
    /// per row.
    #[test]
    fn wrapped_link_reinterned_per_row_keeps_one_id() {
        let mut interner = HyperlinkInterner::new();
        let ids: HashSet<HyperlinkId> = (0..3)
            .map(|_| interner.intern(source("7", "https://a.example/very/long/path")))
            .collect();
        assert_eq!(ids.len(), 1);
    }

    /// Asserts that many distinct keys all receive unique ids.
    ///
    /// Case: `ls --hyperlink=auto` fills a screen with one link per file.
    #[test]
    fn distinct_keys_never_share_an_id() {
        let mut interner = HyperlinkInterner::new();
        let ids: HashSet<HyperlinkId> = (0..32)
            .map(|i| {
                interner.intern(source(
                    &format!("{i}_alacritty"),
                    &format!("file:///tmp/{i}"),
                ))
            })
            .collect();
        assert_eq!(ids.len(), 32);
    }

    /// Asserts that source ids are compared byte for byte rather than
    /// trimmed or canonicalized.
    ///
    /// Case: a program emits `id=42`, `id=042`, and `id=42 ` for three
    /// links.
    #[test]
    fn source_id_is_compared_exactly() {
        let mut interner = HyperlinkInterner::new();
        let ids: HashSet<HyperlinkId> = ["42", "042", "42 "]
            .into_iter()
            .map(|id| interner.intern(source(id, "https://a.example")))
            .collect();
        assert_eq!(ids.len(), 3);
    }

    /// Asserts that fresh keys are numbered from one upwards.
    ///
    /// Case: a fresh session prints its first three links.
    #[test]
    fn new_keys_receive_monotonic_ids_starting_at_one() {
        let mut interner = HyperlinkInterner::new();
        assert_eq!(interner.intern(source("1", "https://a.example")), id(1));
        assert_eq!(interner.intern(source("2", "https://b.example")), id(2));
        assert_eq!(interner.intern(source("3", "https://c.example")), id(3));
    }

    /// Asserts that re-interning a known key leaves the next id untouched.
    ///
    /// Case: a single link covers dozens of cells in a row, so the frame
    /// builder interns it once per cell.
    #[test]
    fn reinterning_an_existing_key_does_not_advance_the_counter() {
        let mut interner = HyperlinkInterner::new();
        interner.intern(source("1", "https://a.example"));
        interner.intern(source("1", "https://a.example"));
        let next = interner.intern(source("2", "https://b.example"));
        assert_eq!(next, id(2));
    }

    /// Asserts that an assigned id never changes as more keys arrive.
    ///
    /// Case: links keep accumulating across frames while earlier cells
    /// are already on screen holding their ids.
    #[test]
    fn existing_ids_are_stable_across_later_interning() {
        let mut interner = HyperlinkInterner::new();
        let first = interner.intern(source("1", "https://a.example"));
        for i in 0..64 {
            interner.intern(source(
                &format!("{i}_alacritty"),
                &format!("https://{i}.example"),
            ));
        }
        assert_eq!(interner.intern(source("1", "https://a.example")), first);
        assert_eq!(
            interner.extract(&first),
            Some(&HyperlinkUri::new("https://a.example".to_owned()))
        );
    }

    /// Asserts that extract returns the uri its id was interned for.
    ///
    /// Case: the frame builder holds a cell's wire id and needs the uri
    /// to emit the `(id, uri)` pair the renderer keeps in its table.
    #[test]
    fn extract_round_trips_the_interned_link() {
        let mut interner = HyperlinkInterner::new();
        let id = interner.intern(source("1", "https://a.example"));
        assert_eq!(
            interner.extract(&id),
            Some(&HyperlinkUri::new("https://a.example".to_owned()))
        );
    }

    /// Asserts that extracting an unassigned id yields None rather than
    /// an unrelated uri or a panic.
    ///
    /// Case: an id from evicted scrollback or another terminal reaches
    /// the lookup.
    #[test]
    fn extract_returns_none_for_an_unknown_id() {
        let mut interner = HyperlinkInterner::new();
        interner.intern(source("1", "https://a.example"));
        assert_eq!(interner.extract(&id(99)), None);
    }

    /// Asserts that two ids sharing a uri both resolve back to it.
    ///
    /// Case: the same URL appears twice on screen as two independent
    /// links.
    #[test]
    fn distinct_ids_for_one_uri_both_resolve_to_it() {
        let mut interner = HyperlinkInterner::new();
        let first = interner.intern(source("0_alacritty", "https://a.example"));
        let second = interner.intern(source("1_alacritty", "https://a.example"));
        let expected = HyperlinkUri::new("https://a.example".to_owned());
        assert_ne!(first, second);
        assert_eq!(interner.extract(&first), Some(&expected));
        assert_eq!(interner.extract(&second), Some(&expected));
    }

    /// Asserts that an empty uri is interned like any other value rather
    /// than rejected.
    ///
    /// Case: a VT backend hands the interner an empty uri.
    #[test]
    fn empty_uri_is_interned_without_special_casing() {
        let mut interner = HyperlinkInterner::new();
        let id = interner.intern(source("1", ""));
        assert_eq!(
            interner.extract(&id),
            Some(&HyperlinkUri::new(String::new()))
        );
    }

    /// Asserts that a long uri round-trips whole and stays distinct from its prefix.
    ///
    /// Case: an application prints a generated URL carrying a
    /// multi-kilobyte query string.
    #[test]
    fn long_uri_is_stored_without_truncation() {
        let mut interner = HyperlinkInterner::new();
        let long = format!("https://a.example/?q={}", "x".repeat(4096));
        let longer = format!("{long}y");
        let first = interner.intern(source("1", &long));
        let second = interner.intern(source("1", &longer));
        assert_ne!(first, second);
        assert_eq!(interner.extract(&first), Some(&HyperlinkUri::new(long)));
        assert_eq!(interner.extract(&second), Some(&HyperlinkUri::new(longer)));
    }

    /// Asserts that uris differing only in encoding or case stay distinct
    /// rather than being percent-decoded or case-folded.
    ///
    /// Case: an application prints links that differ only in
    /// percent-encoding or letter case, such as `https://例.jp/%E3%81%82`
    /// and `https://例.jp/あ`.
    #[test]
    fn uri_bytes_are_preserved_without_normalization() {
        let mut interner = HyperlinkInterner::new();
        let variants = [
            "https://例.jp/%E3%81%82",
            "https://例.jp/あ",
            "https://EXAMPLE.jp/a",
            "https://example.jp/a",
        ];
        let ids: HashSet<HyperlinkId> = variants
            .into_iter()
            .map(|uri| interner.intern(source("1", uri)))
            .collect();
        assert_eq!(ids.len(), variants.len());
        for uri in variants {
            let id = interner.intern(source("1", uri));
            assert_eq!(
                interner.extract(&id),
                Some(&HyperlinkUri::new(uri.to_owned()))
            );
        }
    }

    /// Asserts that the scheme allowlist accepts the four canonical
    /// schemes regardless of letter case.
    ///
    /// Case: a shell emits an OSC 8 link whose scheme the remote program
    /// spelled `HTTPS:` rather than `https:`.
    #[test]
    fn is_allowed_accepts_canonical_schemes_case_insensitive() {
        assert!(is_allowed("http://example.com"));
        assert!(is_allowed("HTTPS://example.com"));
        assert!(is_allowed("Mailto:foo@example"));
        assert!(is_allowed("ftp://example.com"));
    }

    /// Asserts that the scheme allowlist rejects dangerous, unknown, and
    /// malformed inputs rather than falling back to permitting them.
    ///
    /// Case: a hostile program prints an OSC 8 link with a `javascript:`
    /// target, hoping the terminal will hand it to the OS opener.
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

    /// Asserts that two opens sharing a nonempty id and a uri return one id.
    ///
    /// Case: a build tool prints the same error link at the top and the
    /// bottom of its output, tagging both with `id=err1`.
    #[test]
    fn open_with_one_id_and_uri_returns_the_same_id_twice() {
        let mut interner = HyperlinkInterner::new();
        let first = interner.open(
            Some("err1".to_owned()),
            HyperlinkUri::new("https://a.example"),
        );
        let second = interner.open(
            Some("err1".to_owned()),
            HyperlinkUri::new("https://a.example"),
        );
        assert_eq!(first, second);
    }

    /// Asserts that two opens without an id return distinct ids even for
    /// one uri.
    ///
    /// Case: `ls --hyperlink=auto` lists the same file twice, and neither
    /// listing carries an id.
    #[test]
    fn open_without_an_id_returns_a_fresh_id_each_time() {
        let mut interner = HyperlinkInterner::new();
        let first = interner.open(None, HyperlinkUri::new("https://a.example"));
        let second = interner.open(None, HyperlinkUri::new("https://a.example"));
        assert_ne!(first, second);
    }

    /// Asserts that an empty id is treated as no id at all.
    ///
    /// Case: a script interpolates an unset shell variable into its
    /// `id=` parameter and prints two links to one page.
    #[test]
    fn open_with_an_empty_id_returns_a_fresh_id_each_time() {
        let mut interner = HyperlinkInterner::new();
        let first = interner.open(Some(String::new()), HyperlinkUri::new("https://a.example"));
        let second = interner.open(Some(String::new()), HyperlinkUri::new("https://a.example"));
        assert_ne!(first, second);
    }

    /// Asserts that an id opened without a key still resolves to its uri.
    ///
    /// Case: the frame builder looks up the uri of a link the program
    /// printed without an id.
    #[test]
    fn an_id_opened_without_a_key_resolves_to_its_uri() {
        let mut interner = HyperlinkInterner::new();
        let uri = HyperlinkUri::new("https://a.example");
        let id = interner.open(None, uri.clone());
        assert_eq!(interner.extract(&id), Some(&uri));
    }

    /// Asserts that one id reused across two uris yields distinct ids.
    ///
    /// Case: a long-running program reuses `id=1` for an unrelated second
    /// URL later in its output.
    #[test]
    fn open_reusing_an_id_for_another_uri_returns_a_distinct_id() {
        let mut interner = HyperlinkInterner::new();
        let first = interner.open(Some("1".to_owned()), HyperlinkUri::new("https://a.example"));
        let second = interner.open(Some("1".to_owned()), HyperlinkUri::new("https://b.example"));
        assert_ne!(first, second);
    }
}
