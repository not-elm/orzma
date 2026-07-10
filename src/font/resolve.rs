//! Resolves a font-family name and face attributes to font-file bytes plus a
//! collection face index, via a `fontique` collection. Pure over the borrowed
//! collection + source cache, so tests inject a collection preloaded from known
//! font files instead of relying on the host's installed fonts.

use fontique::{
    Attributes, Collection, FontStyle, FontWeight, FontWidth, QueryFamily, QueryStatus, SourceCache,
};
use orzma_configs::font::{FontSlant, FontStyleSpec};
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

/// Maps a parsed `FontStyleSpec` to fontique query attributes (normal width).
pub(super) fn attributes_of(spec: FontStyleSpec) -> Attributes {
    let style = match spec.slant {
        FontSlant::Normal => FontStyle::Normal,
        FontSlant::Italic => FontStyle::Italic,
        FontSlant::Oblique => FontStyle::Oblique(None),
    };
    Attributes {
        width: FontWidth::NORMAL,
        style,
        weight: FontWeight::new(f32::from(spec.weight)),
    }
}

/// The configured family did not resolve to a usable face — either its name is
/// absent from the collection (system font DB), or it is present but no face
/// matched the query.
#[derive(Debug)]
pub(super) struct FamilyNotFound;

/// Resolves a *configured* face, distinguishing an absent family (`Err`) from a
/// present family (fontique returns its closest match for the attributes). Looks
/// up `family_id` first because `Query::set_families` silently skips unknown
/// names, which `resolve_face_bytes` alone reports only as `None`.
pub(super) fn resolve_configured_face(
    collection: &mut Collection,
    source_cache: &mut SourceCache,
    family: &str,
    attributes: Attributes,
) -> Result<(Vec<u8>, u32), FamilyNotFound> {
    if collection.family_id(family).is_none() {
        return Err(FamilyNotFound);
    }
    resolve_face_bytes(collection, source_cache, family, attributes).ok_or(FamilyNotFound)
}

/// Resolves `family` at `attributes` to `(font-file bytes, .ttc face index)`,
/// or `None` when no face resolves — either the family name is absent from the
/// collection, or it is present but the query returns no match. A present family
/// that merely lacks the exact requested weight/style still returns `Some` (the
/// closest match). `resolve_configured_face` maps a `None` here to `FamilyNotFound`.
fn resolve_face_bytes(
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

    fn deterministic_collection() -> (Collection, SourceCache) {
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
        let (mut collection, mut source_cache) = deterministic_collection();
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
        let (mut collection, mut source_cache) = deterministic_collection();
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

    #[test]
    fn attributes_of_maps_weight_and_slant() {
        let a = attributes_of(FontStyleSpec {
            weight: 600,
            slant: FontSlant::Italic,
        });
        assert_eq!(a.weight, FontWeight::new(600.0));
        assert_eq!(a.style, FontStyle::Italic);
        assert_eq!(a.width, FontWidth::NORMAL);
    }

    #[test]
    fn resolve_configured_face_errors_on_absent_family() {
        let (mut collection, mut source_cache) = deterministic_collection();
        let r = resolve_configured_face(
            &mut collection,
            &mut source_cache,
            "no-such-family-4d1",
            attributes_of(FontStyleSpec {
                weight: 400,
                slant: FontSlant::Normal,
            }),
        );
        assert!(r.is_err());
    }

    #[test]
    fn resolve_configured_face_returns_bytes_for_registered_family() {
        let (mut collection, mut source_cache) = deterministic_collection();
        let blob = Blob::new(Arc::new(bundled::REGULAR) as Arc<dyn AsRef<[u8]> + Send + Sync>);
        let registered = collection.register_fonts(blob, None);
        let family = collection.family_name(registered[0].0).unwrap().to_string();
        let (bytes, _index) = resolve_configured_face(
            &mut collection,
            &mut source_cache,
            &family,
            attributes_of(FontStyleSpec {
                weight: 400,
                slant: FontSlant::Normal,
            }),
        )
        .expect("registered family resolves");
        assert!(!bytes.is_empty());
    }
}
