//! Resolves a font-family name and face attributes to font-file bytes plus a
//! collection face index, via a `fontique` collection. Pure over the borrowed
//! collection + source cache, so tests inject a collection preloaded from known
//! font files instead of relying on the host's installed fonts.

use fontique::{
    Attributes, Collection, FontStyle, FontWeight, FontWidth, QueryFamily, QueryStatus, SourceCache,
};
use orzma_tty_renderer::FontFace;

/// Returns the fontique query attributes (weight + style, normal width) for
/// `face`.
pub(super) fn face_attributes(face: FontFace) -> Attributes {
    let (weight, style) = match face {
        FontFace::Regular => (FontWeight::NORMAL, FontStyle::Normal),
        FontFace::Bold => (FontWeight::BOLD, FontStyle::Normal),
        FontFace::Italic => (FontWeight::NORMAL, FontStyle::Italic),
        FontFace::BoldItalic => (FontWeight::BOLD, FontStyle::Italic),
    };
    Attributes {
        width: FontWidth::NORMAL,
        style,
        weight,
    }
}

/// Resolves `family` at `attributes` to `(font-file bytes, .ttc face index)`,
/// or `None` when the family name is absent from the collection. A family that
/// is present but lacks the requested weight/style still returns `Some` (the
/// closest match) — the caller substitutes bundled bytes only on `None`.
pub(super) fn resolve_face_bytes(
    collection: &mut Collection,
    source_cache: &mut SourceCache,
    family: &str,
    attributes: Attributes,
) -> Option<(Vec<u8>, u32)> {
    let mut query = collection.query(source_cache);
    query.set_families([QueryFamily::Named(family)]);
    query.set_attributes(attributes);
    let mut resolved = None;
    query.matches_with(|font| {
        resolved = Some((font.blob.as_ref().to_vec(), font.index));
        QueryStatus::Stop
    });
    resolved
}

#[cfg(test)]
mod tests {
    use super::*;
    use fontique::{Blob, CollectionOptions};
    use orzma_tty_renderer::bundled;
    use std::sync::Arc;

    fn collection() -> (Collection, SourceCache) {
        // Deterministic + fast: skip the host font scan. After Task 2,
        // CollectionOptions::default() has system_fonts: true.
        (
            Collection::new(CollectionOptions {
                system_fonts: false,
                ..Default::default()
            }),
            SourceCache::new(Default::default()),
        )
    }

    #[test]
    fn registered_family_resolves_to_bytes() {
        let (mut collection, mut source_cache) = collection();
        let blob = Blob::new(Arc::new(bundled::REGULAR) as Arc<dyn AsRef<[u8]> + Send + Sync>);
        let registered = collection.register_fonts(blob, None);
        let family_id = registered.first().expect("at least one family").0;
        let family = collection
            .family_name(family_id)
            .expect("family name")
            .to_string();

        let resolved = resolve_face_bytes(
            &mut collection,
            &mut source_cache,
            &family,
            face_attributes(FontFace::Regular),
        );
        let (bytes, _index) = resolved.expect("registered family must resolve");
        assert!(!bytes.is_empty());
    }

    #[test]
    fn absent_family_returns_none() {
        let (mut collection, mut source_cache) = collection();
        let resolved = resolve_face_bytes(
            &mut collection,
            &mut source_cache,
            "no-such-family-9c2f1a",
            face_attributes(FontFace::Regular),
        );
        assert!(resolved.is_none());
    }

    #[test]
    fn attributes_differ_by_face() {
        assert_eq!(
            face_attributes(FontFace::Regular).weight,
            FontWeight::NORMAL
        );
        assert_eq!(face_attributes(FontFace::Bold).weight, FontWeight::BOLD);
        assert_eq!(face_attributes(FontFace::Italic).style, FontStyle::Italic);
        assert_eq!(face_attributes(FontFace::Regular).style, FontStyle::Normal);
    }
}
