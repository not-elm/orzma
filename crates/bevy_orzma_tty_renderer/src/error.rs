//! The error type the terminal renderer reports, and the result alias
//! built on it.

use crate::font::FontFace;
use ab_glyph::InvalidFont;
use orzma_vt::prelude::VtError;
use thiserror::Error;

/// A `Result` whose error is [`RendererError`].
pub type RendererResult<T = ()> = Result<T, RendererError>;

/// Every failure the terminal renderer reports.
#[derive(Debug, Error)]
pub enum RendererError {
    /// The bytes supplied for a font face that `ab_glyph` cannot parse.
    #[error("ab_glyph rejected {face:?} face: {source}")]
    FontParse {
        /// The face whose bytes were invalid.
        face: FontFace,
        /// The parser's refusal.
        #[source]
        source: InvalidFont,
    },
    /// A frame whose content the VT schema rejects.
    #[error(transparent)]
    Vt(#[from] VtError),
    /// A pane's cell or glyph `ShaderBuffer` asset that is missing, so the
    /// pane cannot be uploaded.
    #[error("a terminal's cell or glyph shader buffer is missing")]
    MissingShaderBuffer,
}
