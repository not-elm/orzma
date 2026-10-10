//! The error type the terminal renderer reports, and the result alias built on it.

use orzma_vt::prelude::VtError;
use thiserror::Error;

/// A `Result` whose error is [`RendererError`].
pub type RendererResult<T = ()> = Result<T, RendererError>;

/// Every failure the terminal renderer reports.
#[derive(Debug, Error)]
pub enum RendererError {
    /// Font bytes that are not a font, or that hold no face at the
    /// requested `.ttc` index.
    #[error("failed to parse the font")]
    FontParse,
    /// A frame whose content the VT schema rejects.
    #[error(transparent)]
    Vt(#[from] VtError),
    /// A pane's cell or glyph `ShaderBuffer` asset that is missing, so the
    /// pane cannot be uploaded.
    #[error("a terminal's cell or glyph shader buffer is missing")]
    MissingShaderBuffer,
}
