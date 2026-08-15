use crate::schema::cell::SourceHyperlink;
use std::collections::HashMap;

pub struct HyperlinkInterner {
    id: u32,
    id_to_uri: HashMap<HyperlinkId, HyperlinkUri>,
    source_to_id: HashMap<SourceHyperlink, HyperlinkId>,
}

impl HyperlinkInterner {
    /// Constructs an empty interner.
    ///
    /// The first id handed out is `HyperlinkId(1)`. `HyperlinkId(0)` is
    /// reserved as the "no hyperlink" sentinel across the wire, the CPU
    /// grid, and GPU storage.
    pub fn new() -> Self {
        Self {
            id: 1,
            id_to_uri: HashMap::new(),
            source_to_id: HashMap::new(),
        }
    }

    pub fn intern(&mut self, source: SourceHyperlink) -> HyperlinkId {
        if let Some(id) = self.source_to_id.get(&source) {
            return *id;
        }
        let next_id = self.id;
        let next_id = HyperlinkId(next_id);
        self.id += 1;
        self.id_to_uri.insert(next_id, source.uri.clone());
        self.source_to_id.insert(source, next_id);
        next_id
    }

    #[inline]
    pub fn extract(&self, id: &HyperlinkId) -> Option<&HyperlinkUri> {
        self.id_to_uri.get(id)
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, PartialOrd, Ord, Hash)]
pub struct HyperlinkId(u32);

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct HyperlinkUri(pub String);

pub struct Hyperlink {
    pub id: HyperlinkId,
}

impl Default for HyperlinkInterner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::cell::HyperlinkSourceId;
    use std::collections::HashSet;

    fn source(id: &str, uri: &str) -> SourceHyperlink {
        SourceHyperlink {
            id: HyperlinkSourceId::new(id.to_owned()),
            uri: HyperlinkUri(uri.to_owned()),
        }
    }

    /// Asserts that interning an equal key twice returns the same id.
    ///
    /// Case: one OSC 8 link spans many cells of a row, and the frame
    /// builder builds a fresh key for every cell it walks. Two keys
    /// constructed separately from the same source id and uri must
    /// therefore resolve to one wire id.
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
    /// `1_alacritty`. They are two separate links on screen, and
    /// hovering one must not underline the other.
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
    /// OSC 8 requires that cells pointing at different URIs are never
    /// underlined together, and collapsing the two would also make a
    /// click open the wrong address.
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
    /// per row. Every wrapped fragment must join one hover group.
    #[test]
    fn wrapped_link_reinterned_per_row_keeps_one_id() {
        let mut interner = HyperlinkInterner::new();
        let ids: HashSet<HyperlinkId> = (0..3)
            .map(|_| interner.intern(source("7", "https://a.example/very/long/path")))
            .collect();
        assert_eq!(ids.len(), 1);
    }

    /// Asserts that many distinct keys all receive unique non-zero ids.
    ///
    /// Case: `ls --hyperlink=auto` fills a screen with one link per file.
    /// Hovering a file name must not underline its neighbours, so no two
    /// keys may collapse onto one wire id.
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
        assert!(!ids.contains(&HyperlinkId(0)));
    }

    /// Asserts that source ids are compared byte for byte.
    ///
    /// Case: a program emits `id=42`, `id=042`, and `id=42 ` for three
    /// links. Interpreting OSC 8 parameters belongs to the VT layer, so
    /// the interner treats what it is handed as opaque bytes instead of
    /// trimming or canonicalizing it.
    #[test]
    fn source_id_is_compared_exactly() {
        let mut interner = HyperlinkInterner::new();
        let ids: HashSet<HyperlinkId> = ["42", "042", "42 "]
            .into_iter()
            .map(|id| interner.intern(source(id, "https://a.example")))
            .collect();
        assert_eq!(ids.len(), 3);
    }

    /// Asserts that no key is ever assigned the reserved zero id.
    ///
    /// Case: the GPU cell attribute stores `0` to mean "this cell carries
    /// no link". Handing out `0` for a real link would paint the hover
    /// underline across unlinked cells.
    #[test]
    fn intern_never_returns_the_zero_sentinel() {
        let mut interner = HyperlinkInterner::new();
        for i in 0..16 {
            let id = interner.intern(source(&format!("{i}"), &format!("https://{i}.example")));
            assert_ne!(id, HyperlinkId(0));
        }
    }

    /// Asserts that fresh keys are numbered from one upwards.
    ///
    /// Case: the first link a session prints must not land on the zero
    /// sentinel, and a replayed sequence of links must reproduce the same
    /// wire ids.
    #[test]
    fn new_keys_receive_monotonic_ids_starting_at_one() {
        let mut interner = HyperlinkInterner::new();
        assert_eq!(
            interner.intern(source("1", "https://a.example")),
            HyperlinkId(1)
        );
        assert_eq!(
            interner.intern(source("2", "https://b.example")),
            HyperlinkId(2)
        );
        assert_eq!(
            interner.intern(source("3", "https://c.example")),
            HyperlinkId(3)
        );
    }

    /// Asserts that re-interning a known key leaves the next id untouched.
    ///
    /// Case: a single link covers dozens of cells in a row, so the frame
    /// builder interns it once per cell. Advancing the counter per cell
    /// would burn the id space within one screen and fill the wire table
    /// with duplicates.
    #[test]
    fn reinterning_an_existing_key_does_not_advance_the_counter() {
        let mut interner = HyperlinkInterner::new();
        interner.intern(source("1", "https://a.example"));
        interner.intern(source("1", "https://a.example"));
        let next = interner.intern(source("2", "https://b.example"));
        assert_eq!(next, HyperlinkId(2));
    }

    /// Asserts that an assigned id never changes as more keys arrive.
    ///
    /// Case: links keep accumulating across frames while earlier cells
    /// are already on screen holding their ids. Renumbering an existing
    /// entry would silently desynchronize hover grouping from the wire
    /// table the renderer already stored.
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
            Some(&HyperlinkUri("https://a.example".to_owned()))
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
            Some(&HyperlinkUri("https://a.example".to_owned()))
        );
    }

    /// Asserts that extracting an unassigned id yields None.
    ///
    /// Case: an id from evicted scrollback or another terminal reaches
    /// the lookup. Resolving it to some unrelated uri, or panicking, are
    /// both worse than reporting that the link is gone.
    #[test]
    fn extract_returns_none_for_an_unknown_id() {
        let mut interner = HyperlinkInterner::new();
        interner.intern(source("1", "https://a.example"));
        assert_eq!(interner.extract(&HyperlinkId(99)), None);
    }

    /// Asserts that the reserved zero id resolves to nothing.
    ///
    /// Case: a caller forwards an unlinked cell's `0` straight into the
    /// lookup. If the sentinel resolved to a uri, cells carrying no link
    /// at all would become clickable.
    #[test]
    fn extract_returns_none_for_the_zero_sentinel() {
        let mut interner = HyperlinkInterner::new();
        interner.intern(source("1", "https://a.example"));
        assert_eq!(interner.extract(&HyperlinkId(0)), None);
    }

    /// Asserts that two ids sharing a uri both resolve back to it.
    ///
    /// Case: the same URL appears twice on screen as two independent
    /// links. They highlight separately on hover, yet clicking either one
    /// must open the same address.
    #[test]
    fn distinct_ids_for_one_uri_both_resolve_to_it() {
        let mut interner = HyperlinkInterner::new();
        let first = interner.intern(source("0_alacritty", "https://a.example"));
        let second = interner.intern(source("1_alacritty", "https://a.example"));
        let expected = HyperlinkUri("https://a.example".to_owned());
        assert_ne!(first, second);
        assert_eq!(interner.extract(&first), Some(&expected));
        assert_eq!(interner.extract(&second), Some(&expected));
    }

    /// Asserts that an empty uri is interned like any other value.
    ///
    /// Case: the alacritty backend never produces one, because
    /// `OSC 8 ; ; ST` terminates a link and yields no hyperlink at all.
    /// Another backend may hand one over, and the agreed policy is that
    /// judging a uri belongs to the VT layer, so the interner stores it
    /// rather than rejecting it or folding it onto the zero sentinel.
    #[test]
    fn empty_uri_is_interned_without_special_casing() {
        let mut interner = HyperlinkInterner::new();
        let id = interner.intern(source("1", ""));
        assert_ne!(id, HyperlinkId(0));
        assert_eq!(interner.extract(&id), Some(&HyperlinkUri(String::new())));
    }

    /// Asserts that a long uri round-trips whole and stays distinct from its prefix.
    ///
    /// Case: an application prints a generated URL carrying a
    /// multi-kilobyte query string. Truncating it, or matching two such
    /// URLs on a shared prefix, would send a click to the wrong address.
    /// Any length ceiling belongs to the VT layer, not to the interner.
    #[test]
    fn long_uri_is_stored_without_truncation() {
        let mut interner = HyperlinkInterner::new();
        let long = format!("https://a.example/?q={}", "x".repeat(4096));
        let longer = format!("{long}y");
        let first = interner.intern(source("1", &long));
        let second = interner.intern(source("1", &longer));
        assert_ne!(first, second);
        assert_eq!(interner.extract(&first), Some(&HyperlinkUri(long)));
        assert_eq!(interner.extract(&second), Some(&HyperlinkUri(longer)));
    }

    /// Asserts that uris differing only in encoding or case stay distinct.
    ///
    /// Case: a terminal carries the bytes an application printed. Percent
    /// decoding or case folding here would merge `https://例.jp/%E3%81%82`
    /// with `https://例.jp/あ`, which the interner has no authority to
    /// treat as one resource.
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
            assert_eq!(interner.extract(&id), Some(&HyperlinkUri(uri.to_owned())));
        }
    }
}
