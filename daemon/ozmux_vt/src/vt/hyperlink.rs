//! OSC 8 hyperlink interning.
//!
//! Maps alacritty `(id, uri)` pairs to monotonic `HyperlinkId` wire
//! ids. The wire id type comes from `ozmux_vt::frame`.
//!
//! Keying by `(alac_id, uri)` matches xterm.js' `OscLinkService`
//! convention: OSC 8 lets two unrelated links share the same explicit
//! id, so keying by alacritty id alone would collide them.

use crate::frame::{HyperlinkId, HyperlinkUri};

/// Alacritty-side hyperlink identifier (OSC 8 `id=` parameter or
/// auto-generated `"N_alacritty"`). Not transported on the wire —
/// only used as part of the interner key.
#[derive(Clone, Eq, PartialEq, Hash, Debug)]
struct AlacrittyHyperlinkId(String);

/// Interns `(alacritty_id, uri)` pairs to monotonic [`HyperlinkId`].
///
/// Linear scan — realistic sessions carry ≤100 distinct hyperlinks
/// (e.g., one per file in `ls --hyperlink=auto`), so `HashMap`
/// allocation overhead exceeds its lookup savings.
pub struct HyperlinkInterner {
    entries: Vec<((AlacrittyHyperlinkId, HyperlinkUri), HyperlinkId)>,
    next_id: HyperlinkId,
}

impl HyperlinkInterner {
    /// Constructs an empty interner. The first id assigned is
    /// `HyperlinkId(1)`; `HyperlinkId(0)` is reserved as the
    /// universal "no hyperlink" sentinel across the wire, the CPU
    /// grid, and GPU storage.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            next_id: HyperlinkId(1),
        }
    }

    /// Returns the wire id for `(alac_id, uri)`, assigning a new
    /// monotonic id if the pair has not been seen before.
    pub fn intern(&mut self, alac_id: &str, uri: &str) -> HyperlinkId {
        for ((a, u), id) in &self.entries {
            if a.0 == alac_id && u.as_str() == uri {
                return *id;
            }
        }
        let id = self.next_id;
        self.next_id = HyperlinkId(
            self.next_id
                .0
                .checked_add(1)
                .expect("hyperlink id space exhausted"),
        );
        self.entries.push((
            (
                AlacrittyHyperlinkId(alac_id.to_owned()),
                HyperlinkUri::new(uri.to_owned()),
            ),
            id,
        ));
        id
    }
}

impl Default for HyperlinkInterner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_same_id_for_repeated_key() {
        let mut interner = HyperlinkInterner::new();
        let a = interner.intern("42", "https://a.example");
        let b = interner.intern("42", "https://a.example");
        assert_eq!(a, b);
    }

    #[test]
    fn assigns_monotonic_ids_for_new_keys() {
        let mut interner = HyperlinkInterner::new();
        let a = interner.intern("42", "https://a.example");
        let b = interner.intern("42", "https://b.example");
        let c = interner.intern("99", "https://a.example");
        assert_eq!(a, HyperlinkId(1));
        assert_eq!(b, HyperlinkId(2));
        assert_eq!(c, HyperlinkId(3));
    }

    #[test]
    fn never_assigns_hyperlink_id_zero() {
        let mut interner = HyperlinkInterner::new();
        for i in 0..16 {
            let id = interner.intern(&format!("{i}"), &format!("https://{i}.example"));
            assert_ne!(
                id,
                HyperlinkId(0),
                "id 0 is reserved as the no-link sentinel"
            );
        }
    }

    #[test]
    fn keys_by_id_and_uri_compound() {
        // xterm OscLinkService convention: same id with different uri
        // = distinct.
        let mut interner = HyperlinkInterner::new();
        let same_id_diff_uri_a = interner.intern("foo", "https://a.example");
        let same_id_diff_uri_b = interner.intern("foo", "https://b.example");
        assert_ne!(same_id_diff_uri_a, same_id_diff_uri_b);
    }
}
