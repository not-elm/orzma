//! Grid viewport vocabulary: [`DisplayOffset`] and [`GridSize`].

/// Number of scrollback rows the viewport sits above the live tail.
///
/// `0` means the viewport is pinned to the live tail; a positive value
/// counts the scrollback rows showing above it. The unit is grid rows.
///
/// # Invariants
///
/// The producing backend keeps the value within the scrollback
/// capacity and at `0` while the alternate screen is active; this type
/// does not enforce either bound itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayOffset(pub u32);

/// Grid dimensions in cells.
///
/// The row count is the source of truth for "one screenful" (scroll
/// paging) and for verifying an applied resize.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridSize {
    /// Visible column count.
    pub cols: u16,
    /// Visible row count.
    pub rows: u16,
}
