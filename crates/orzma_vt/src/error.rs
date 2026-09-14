//! The error type the VT reports, and the result alias built on it.

use crate::screen::grid::GridSize;
use thiserror::Error;

/// A `Result` whose error is [`VtError`].
pub type VtResult<T = ()> = Result<T, VtError>;

/// Every failure the VT reports.
#[derive(Debug, Error)]
pub enum VtError {
    /// A column and row count that is not a valid [`GridSize`].
    #[error(transparent)]
    GridSize(#[from] GridSizeError),
    /// A glyph a row refused to stamp.
    #[error(transparent)]
    Stamp(#[from] StampError),
    /// A run whose widths do not describe its text.
    #[error(transparent)]
    Run(#[from] RunError),
}

/// The reason a row refuses to stamp a glyph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum StampError {
    /// A glyph, or the continuation of a wide glyph, that would land
    /// past the end of the row.
    #[error("a stamp reaches past the end of the row")]
    OutOfRow,
}

/// The reason a column and row count is not a valid [`GridSize`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum GridSizeError {
    /// A column or row count of zero.
    #[error("a grid axis is zero")]
    ZeroAxis,
    /// A column or row count above [`GridSize::MAX_COLS`] /
    /// [`GridSize::MAX_ROWS`].
    #[error("a grid axis exceeds {}x{}", GridSize::MAX_COLS, GridSize::MAX_ROWS)]
    TooLarge,
}

/// The reason a run's widths do not describe its text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum RunError {
    /// A widths list whose length differs from the `char` count of the
    /// text.
    #[error("a run's widths do not cover each char of its text")]
    WidthCount,
    /// A widths list whose sum differs from the run's column span.
    #[error("a run's widths do not sum to its columns")]
    WidthSum,
    /// A width other than 0, 1 or 2.
    #[error("a run width is not 0, 1 or 2")]
    InvalidWidth,
    /// A first `char` at width zero, with no glyph before it to join.
    #[error("a run starts with a continuation")]
    LeadingContinuation,
}
