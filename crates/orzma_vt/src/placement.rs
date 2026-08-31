//! Webview placement vocabulary: the id a mount is addressed by, the
//! rectangle it reserves, the grid-space geometry an emitted frame
//! carries, and the per-terminal cap.
//!
//! The table itself belongs to each `Screen`; see
//! [`crate::screen::placements`].

use crate::screen::grid::coords::GridPoint;

/// VT-assigned identity of one mounted webview placement.
///
/// # Invariants
///
/// Ids are minted monotonically per terminal and never reused within a
/// session, so a delayed id-addressed lifecycle event can never target
/// a successor placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PlacementId(pub u64);

/// One placement's grid-space geometry at emit time.
///
/// The point is in active-grid coordinates and does not move when the
/// user scrolls, the same way a cursor point or a selection endpoint
/// does not; the consumer projects it with the frame's display offset.
///
/// # Invariants
///
/// `size` always equals the mount-time reservation for `id`; the VT
/// treats a size change as a remount under a fresh id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnchoredPlacement {
    /// The placement this geometry belongs to.
    pub id: PlacementId,
    /// Active-grid cell the rect's top-left corner sits at.
    pub point: GridPoint,
    /// The rect's extent, unchanged from the mount that reserved it.
    pub size: PlacementSize,
}

/// The cell rectangle a mount reserves, without its position.
///
/// This is deliberately not [`GridSize`](crate::prelude::GridSize), whose row count is the source
/// of truth for one screenful; a placement's reservation is a sub-rectangle
/// and must not be substitutable for a grid dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlacementSize {
    /// Reserved height in cells.
    pub rows: u16,
    /// Reserved width in cells.
    pub cols: u16,
}

/// Upper bound on live placements per terminal, across both screens.
///
/// It matches the renderer's overlay slot count, so a mount the VT
/// accepts is always one the host can place. The two are not mirrors:
/// the host allocates slots per terminal among live children, while this
/// cap counts both screens, so it is strictly the stricter of the two.
pub const MAX_PLACEMENTS: usize = 12;
