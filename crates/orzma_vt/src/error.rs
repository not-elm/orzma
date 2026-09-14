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
    /// A cell a row refused to stamp.
    #[error(transparent)]
    Stamp(#[from] StampError),
}

/// The reason a row refuses to stamp a cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum StampError {
    /// A cell whose width is a continuation or a filler rather than a
    /// glyph body.
    #[error("a stamp carries a continuation or a filler, not a glyph body")]
    NotABody,
    /// A body, or the continuation of a wide body, that would land past
    /// the end of the row.
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
