//! Webview placement table: id minting, anchor tracking, and viewport
//! projection.
//!
//! [`PlacementStore`] is a side table keyed by the grid line a mount
//! anchored to, never a cell variant, so text writes and reflow cannot
//! corrupt a placement. It converts to viewport coordinates only at
//! emit time.

use crate::schema::{DisplayOffset, GridSize, ProjectedPlacement, ScreenKind};

/// The placement table: minted ids, line anchors, and occupancy spans.
// TODO: Carry the id counter, the `(view_id, instance)` index, the
// per-line occupancy spans, and the anchor bookkeeping `HistoryEvent`
// drives.
pub(crate) struct PlacementStore {}

impl PlacementStore {
    /// Builds an empty store whose first minted id is unused.
    pub fn new() -> Self {
        Self {}
    }

    /// Projects every placement on `active_screen` into viewport
    /// coordinates — the complete list, not a diff.
    ///
    /// # Invariants
    ///
    /// Projection reads; it never evicts, repairs an anchor, or
    /// refreshes a cache. Those belong to `HistoryEvent` handling,
    /// which runs while damage can still be staged — a mutation here
    /// would land after the ledger was drained and reach no frame.
    // TODO: Return the placements anchored on `active_screen` once the
    // table exists. An empty list is the honest answer while nothing can
    // be mounted.
    pub fn project(
        &self,
        _active_screen: ScreenKind,
        _offset: DisplayOffset,
        _size: GridSize,
    ) -> Vec<ProjectedPlacement> {
        Vec::new()
    }
}
