//! The identity of one rasterized sprite in the glyph atlas.

use crate::font::FontFace;
use orzma_vt::prelude::MAX_COMBINING;

/// The identity of one rasterized sprite in the atlas: a glyph at a
/// face and size, in the columns the VT reserved for it, with the marks
/// composed onto it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GlyphKey {
    /// The face the glyph is drawn with.
    pub face: FontFace,
    /// The base character's Unicode scalar value.
    pub codepoint: u32,
    /// The font size in physical pixels.
    pub size_px: u16,
    /// Whether the VT reserved two columns for the glyph.
    pub wide: bool,
    marks: [char; MAX_COMBINING],
}

impl GlyphKey {
    /// A key for a bare glyph in one column, with no marks.
    pub fn new(face: FontFace, codepoint: u32, size_px: u16) -> Self {
        Self {
            face,
            codepoint,
            size_px,
            wide: false,
            marks: ['\0'; MAX_COMBINING],
        }
    }

    /// This key with `wide` recording whether the VT reserved two columns
    /// for the glyph.
    pub fn with_wide(mut self, wide: bool) -> Self {
        self.wide = wide;
        self
    }

    /// This key with the first [`MAX_COMBINING`] of `marks` composed
    /// onto its glyph; any further marks are dropped.
    pub fn with_marks(mut self, marks: impl IntoIterator<Item = char>) -> Self {
        self.marks = ['\0'; MAX_COMBINING];
        for (slot, mark) in self.marks.iter_mut().zip(marks) {
            *slot = mark;
        }
        self
    }

    /// The marks composed onto the glyph, in arrival order.
    pub fn marks(self) -> impl Iterator<Item = char> {
        self.marks.into_iter().take_while(|mark| *mark != '\0')
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::iter;

    /// Asserts that a key built without marks carries none, that
    /// `with_marks` keeps at most the cap in arrival order and reports
    /// exactly those marks back, and that marks take part in equality.
    ///
    /// Case: the renderer keys an accented letter and a letter buried
    /// under more marks than one cell retains.
    #[test]
    fn a_key_keeps_at_most_the_capped_marks() {
        let plain = GlyphKey::new(FontFace::Regular, u32::from('e'), 24);
        assert_eq!(plain.marks().count(), 0);
        let accented = plain.with_marks(['\u{0301}', '\u{0302}']);
        assert_eq!(
            accented.marks().collect::<Vec<_>>(),
            ['\u{0301}', '\u{0302}']
        );
        let flooded = plain.with_marks(iter::repeat_n('\u{0301}', MAX_COMBINING + 3));
        assert_eq!(flooded.marks().count(), MAX_COMBINING);
        assert_ne!(plain, accented);
        assert_eq!(plain, GlyphKey::new(FontFace::Regular, u32::from('e'), 24));
    }

    /// Asserts that a new key is narrow, and that the same glyph keyed for
    /// two columns is a different key from the one keyed for one.
    ///
    /// Case: the renderer keys a character once for a wide cell and once
    /// for a narrow cell.
    #[test]
    fn a_wide_key_differs_from_the_same_narrow_key() {
        let narrow = GlyphKey::new(FontFace::Regular, u32::from('あ'), 24);
        assert!(!narrow.wide);

        let wide = narrow.with_wide(true);

        assert!(wide.wide);
        assert_ne!(wide, narrow);
        assert_eq!(wide.with_wide(false), narrow);
    }
}
