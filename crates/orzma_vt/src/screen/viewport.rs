//! Viewport state and its position relative to the live tail.

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DisplayOffset(pub u32);

pub struct ViewportLine(u16);

#[derive(Debug, Default)]
pub struct Viewport {
    pub offset: DisplayOffset,
}
