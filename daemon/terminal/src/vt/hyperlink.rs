//! HyperlinkInterner — maps alacritty `(id, uri)` pairs to monotonic u32 wire ids.
//!
//! See `docs/superpowers/specs/2026-05-14-server-side-vt-phase3b-design.md` § 3.1.
//!
//! Alacritty's `Hyperlink::id()` returns `&str` (e.g., `"42"`, `"foo"`, or
//! auto-generated `"0_alacritty"`). The wire uses `u32` to keep
//! `Run.hyperlink_id` compact. xterm.js' `OscLinkService` keys by `id;;uri`
//! because OSC 8 lets two unrelated links share the same explicit id —
//! keying by alacritty id alone would collide them. This interner therefore
//! keys by `(alac_id, uri)`.

/// Interns `(alacritty_id, uri)` pairs to monotonic u32 wire ids.
///
/// Internal storage is a `Vec<((String, String), u32)>` with linear scan —
/// realistic sessions carry ≤100 distinct hyperlinks (e.g., one per file in
/// `ls --hyperlink=auto`), so `HashMap` allocation overhead exceeds its
/// lookup savings.
pub struct HyperlinkInterner {
    entries: Vec<((String, String), u32)>,
    next_id: u32,
}

impl HyperlinkInterner {
    /// Constructs an empty interner.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            next_id: 0,
        }
    }

    /// Returns the wire id for `(alac_id, uri)`, assigning a new monotonic id
    /// if the pair has not been seen before.
    pub fn intern(&mut self, alac_id: &str, uri: &str) -> u32 {
        for ((a, u), id) in &self.entries {
            if a == alac_id && u == uri {
                return *id;
            }
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.entries
            .push(((alac_id.to_owned(), uri.to_owned()), id));
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
        assert_eq!(a, 0);
        assert_eq!(b, 1);
        assert_eq!(c, 2);
    }

    #[test]
    fn keys_by_id_and_uri_compound() {
        // xterm OscLinkService convention: same id with different uri = distinct.
        let mut interner = HyperlinkInterner::new();
        let same_id_diff_uri_a = interner.intern("foo", "https://a.example");
        let same_id_diff_uri_b = interner.intern("foo", "https://b.example");
        assert_ne!(same_id_diff_uri_a, same_id_diff_uri_b);
    }
}
