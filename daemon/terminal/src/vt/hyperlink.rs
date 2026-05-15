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
//!
//! Three newtypes distinguish the values that flow through this module:
//! `AlacrittyHyperlinkId` (interner-side string id), `HyperlinkUri` (OSC 8
//! target), and `HyperlinkWireId` (monotonic u32 emitted to the wire).

use serde::{Deserialize, Serialize};

/// Wire-level monotonic hyperlink id. Encoded as bare `u32` on the wire via
/// `#[serde(transparent)]`.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HyperlinkWireId(pub u32);

impl HyperlinkWireId {
    /// Returns the underlying `u32`.
    pub fn get(self) -> u32 {
        self.0
    }
}

/// Alacritty-side hyperlink identifier (OSC 8 `id=` parameter or
/// auto-generated `"N_alacritty"`). Not transported on the wire — only used
/// as part of the interner key.
#[derive(Clone, Eq, PartialEq, Hash, Debug)]
pub struct AlacrittyHyperlinkId(String);

impl AlacrittyHyperlinkId {
    /// Wraps a string as an alacritty hyperlink id.
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Returns the underlying string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// OSC 8 hyperlink target URI. Encoded as a bare string on the wire via
/// `#[serde(transparent)]`.
#[derive(Clone, Eq, PartialEq, Hash, Debug, Serialize, Deserialize)]
#[serde(transparent)]
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

/// Interns `(alacritty_id, uri)` pairs to monotonic [`HyperlinkWireId`] values.
///
/// Internal storage is a `Vec<((AlacrittyHyperlinkId, HyperlinkUri), HyperlinkWireId)>`
/// with linear scan — realistic sessions carry ≤100 distinct hyperlinks (e.g.,
/// one per file in `ls --hyperlink=auto`), so `HashMap` allocation overhead
/// exceeds its lookup savings.
pub struct HyperlinkInterner {
    entries: Vec<((AlacrittyHyperlinkId, HyperlinkUri), HyperlinkWireId)>,
    next_id: HyperlinkWireId,
}

impl HyperlinkInterner {
    /// Constructs an empty interner.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            next_id: HyperlinkWireId(0),
        }
    }

    /// Returns the wire id for `(alac_id, uri)`, assigning a new monotonic id
    /// if the pair has not been seen before.
    ///
    /// Argument order is `(alac_id, uri)` — both are `&str`, so transposing them
    /// compiles silently. Call sites must mirror alacritty's `Hyperlink::id()`
    /// then `Hyperlink::uri()` ordering.
    pub fn intern(&mut self, alac_id: &str, uri: &str) -> HyperlinkWireId {
        for ((a, u), id) in &self.entries {
            if a.as_str() == alac_id && u.as_str() == uri {
                return *id;
            }
        }
        let id = self.next_id;
        self.next_id = HyperlinkWireId(self.next_id.0.wrapping_add(1));
        self.entries.push((
            (
                AlacrittyHyperlinkId::new(alac_id.to_owned()),
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
        assert_eq!(a, HyperlinkWireId(0));
        assert_eq!(b, HyperlinkWireId(1));
        assert_eq!(c, HyperlinkWireId(2));
    }

    #[test]
    fn keys_by_id_and_uri_compound() {
        // xterm OscLinkService convention: same id with different uri = distinct.
        let mut interner = HyperlinkInterner::new();
        let same_id_diff_uri_a = interner.intern("foo", "https://a.example");
        let same_id_diff_uri_b = interner.intern("foo", "https://b.example");
        assert_ne!(same_id_diff_uri_a, same_id_diff_uri_b);
    }

    #[test]
    fn wire_id_serializes_transparently_as_u32() {
        let id = HyperlinkWireId(7);
        let bytes = rmp_serde::to_vec(&id).expect("encode");
        let raw: u32 = rmp_serde::from_slice(&bytes).expect("decode u32");
        assert_eq!(raw, 7);
    }

    #[test]
    fn uri_serializes_transparently_as_string() {
        let uri = HyperlinkUri::new("https://example.com");
        let bytes = rmp_serde::to_vec(&uri).expect("encode");
        let raw: String = rmp_serde::from_slice(&bytes).expect("decode String");
        assert_eq!(raw, "https://example.com");
    }
}
