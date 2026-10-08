use orzma_vt::prelude::Style;

/// The weight and style a glyph is drawn in.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum FontFace {
    /// Regular weight, upright style.
    Regular,
    /// Bold weight, upright style.
    Bold,
    /// Regular weight, italic style.
    Italic,
    /// Bold weight, italic style.
    BoldItalic,
}

impl FontFace {
    /// Returns the face that a cell's style selects.
    ///
    /// Only `BOLD` and `ITALIC` take part; every other flag is ignored.
    pub fn from_style(style: Style) -> Self {
        match (style.contains(Style::BOLD), style.contains(Style::ITALIC)) {
            (false, false) => Self::Regular,
            (true, false) => Self::Bold,
            (false, true) => Self::Italic,
            (true, true) => Self::BoldItalic,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::font::FontFace;
    use orzma_vt::prelude::Style;

    /// Asserts that `from_style` picks the face from the `BOLD` and
    /// `ITALIC` flags alone, whatever other `Style` flags the cell carries.
    ///
    /// Case: a program prints bold, italic, and bold-italic text, some of
    /// it also underlined or reversed.
    #[test]
    fn from_style_selects_the_face_from_the_bold_and_italic_bits() {
        let other = Style::all().difference(Style::BOLD | Style::ITALIC);
        let cases = [
            (Style::empty(), FontFace::Regular),
            (Style::BOLD, FontFace::Bold),
            (Style::ITALIC, FontFace::Italic),
            (Style::BOLD | Style::ITALIC, FontFace::BoldItalic),
        ];
        for (style, face) in cases {
            assert_eq!(FontFace::from_style(style), face);
            assert_eq!(FontFace::from_style(style | other), face);
        }
    }
}
