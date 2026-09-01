//! The webview placements one screen owns: the mount table, the
//! resolution of its anchors into grid coordinates, and the eviction
//! sweep.

use crate::placement::{AnchoredPlacement, InstanceId, PlacementSize};
use crate::screen::grid::LineId;
use crate::screen::grid::coords::{GridColumn, GridLine, GridPoint};

/// The placements mounted on one screen.
///
/// Which screen owns the table answers which screen a placement belongs
/// to, so no placement records its own screen.
#[derive(Debug)]
pub(crate) struct ScreenPlacements {
    placements: Vec<Placement>,
}

impl ScreenPlacements {
    /// Builds an empty table.
    pub fn new() -> Self {
        Self {
            placements: Vec::new(),
        }
    }

    /// Number of placements this screen holds.
    pub fn len(&self) -> usize {
        self.placements.len()
    }

    /// Whether this screen holds no placement.
    ///
    /// [`Self::evict_lost_anchors`] returns on this before it touches
    /// the table, which states in one line that a screen holding no
    /// webview resolves no anchor.
    pub fn is_empty(&self) -> bool {
        self.placements.is_empty()
    }

    /// Registers a mount; the caller has already resolved the anchor.
    pub fn mount(&mut self, id: InstanceId, anchor: LineId, col: GridColumn, size: PlacementSize) {
        self.placements.push(Placement {
            id,
            anchor,
            col,
            size,
        });
    }

    /// Drops the placement a re-mount of `id` replaces, without reporting it.
    ///
    /// A superseded id is deliberately unnamed: the re-mount registers a
    /// successor under the same id, and reporting the predecessor as
    /// evicted would tell the host to despawn the live view.
    pub fn supersede(&mut self, id: InstanceId) {
        self.placements.retain(|p| p.id != id);
    }

    /// Removes the placement an `unmount` addresses (`None` removes every
    /// placement on this screen); returns whether anything went.
    pub fn unmount(&mut self, id: Option<InstanceId>) -> bool {
        let before = self.placements.len();
        match id {
            None => self.placements.clear(),
            Some(id) => self.placements.retain(|p| p.id != id),
        }
        before != self.placements.len()
    }

    /// Removes the placements the host names; returns whether anything went.
    /// Ids the table does not hold are ignored.
    pub fn remove_many(&mut self, ids: &[InstanceId]) -> bool {
        let before = self.placements.len();
        self.placements.retain(|p| !ids.contains(&p.id));
        before != self.placements.len()
    }

    /// Resolves every placement's anchor through `line_of` — the complete
    /// list, not a diff.
    ///
    /// # Invariants
    ///
    /// Resolution reads; it never evicts, repairs an anchor, or refreshes
    /// a cache. Those belong to [`Self::evict_lost_anchors`], which runs
    /// while damage can still be staged — a mutation here would land
    /// after the ledger was drained and reach no frame.
    pub fn project(
        &self,
        mut line_of: impl FnMut(LineId) -> Option<GridLine>,
    ) -> Vec<AnchoredPlacement> {
        self.placements
            .iter()
            .filter_map(|p| {
                Some(AnchoredPlacement {
                    id: p.id,
                    point: GridPoint {
                        line: line_of(p.anchor)?,
                        column: p.col,
                    },
                    size: p.size,
                })
            })
            .collect()
    }

    /// Drops the placements `line_of` can no longer resolve and names
    /// them.
    ///
    /// # Invariants
    ///
    /// `line_of` must resolve anchors exactly as the expression handed to
    /// [`Self::project`] does; the matching bound does not enforce it, so
    /// the owning screen passes one expression to both. A placement this
    /// rejects is exactly a placement projection would omit, so no
    /// placement can become unresolvable without also becoming evictable.
    pub fn evict_lost_anchors(
        &mut self,
        mut line_of: impl FnMut(LineId) -> Option<GridLine>,
    ) -> Vec<InstanceId> {
        if self.is_empty() {
            return Vec::new();
        }
        self.evict_where(|p| line_of(p.anchor).is_none())
    }

    /// Empties the table and names every id it held.
    pub fn take_all(&mut self) -> Vec<InstanceId> {
        self.evict_where(|_| true)
    }

    fn evict_where(&mut self, should_evict: impl FnMut(&mut Placement) -> bool) -> Vec<InstanceId> {
        self.placements
            .extract_if(.., should_evict)
            .map(|p| p.id)
            .collect()
    }
}

/// One mounted webview on this screen.
#[derive(Debug)]
struct Placement {
    id: InstanceId,
    anchor: LineId,
    col: GridColumn,
    size: PlacementSize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::grid::Grid;
    use crate::screen::grid::GridSize;
    use crate::screen::grid::coords::ScreenLine;

    fn table() -> ScreenPlacements {
        ScreenPlacements::new()
    }

    fn grid() -> Grid {
        Grid::new(GridSize { cols: 8, rows: 3 }, 10)
    }

    fn mount(table: &mut ScreenPlacements, grid: &Grid, id: u128) {
        table.mount(
            InstanceId(id),
            grid.line_id(ScreenLine::TOP),
            GridColumn(0),
            PlacementSize { rows: 2, cols: 4 },
        );
    }

    /// Asserts that a re-mount of the same id replaces the live
    /// placement instead of stacking a second one beside it.
    ///
    /// Case: a program re-renders the same view after its content
    /// changed.
    #[test]
    fn a_supersede_replaces_the_live_placement_at_the_same_address() {
        let mut table = table();
        let grid = grid();
        mount(&mut table, &grid, 1);
        table.supersede(InstanceId(1));
        mount(&mut table, &grid, 1);
        assert_eq!(table.len(), 1);
        assert_eq!(table.take_all(), vec![InstanceId(1)]);
    }

    /// Asserts that an unmount naming an id removes just that placement and
    /// a `None` id removes every placement on the screen.
    ///
    /// Case: a program tears down one of its views, then exits and asks the
    /// terminal to drop whatever is left.
    #[test]
    fn an_unmount_removes_the_placements_its_address_names() {
        let mut table = table();
        let grid = grid();
        mount(&mut table, &grid, 1);
        mount(&mut table, &grid, 2);
        assert!(table.unmount(Some(InstanceId(1))));
        assert_eq!(table.len(), 1);
        assert!(!table.unmount(Some(InstanceId(1))));
        assert!(table.unmount(None));
        assert!(table.is_empty());
    }

    /// Asserts that a host-driven removal drops exactly the named ids and
    /// reports whether anything went, ignoring ids the table never held.
    ///
    /// Case: a program's control-plane connection drops, and the host clears
    /// the placements its registrations had reserved.
    #[test]
    fn a_host_removal_drops_exactly_the_named_ids() {
        let mut table = table();
        let grid = grid();
        mount(&mut table, &grid, 1);
        mount(&mut table, &grid, 2);
        mount(&mut table, &grid, 3);
        assert!(table.remove_many(&[InstanceId(1), InstanceId(3), InstanceId(9)]));
        assert_eq!(table.len(), 1);
        assert!(!table.remove_many(&[InstanceId(9)]));
    }

    /// Asserts that projection omits the placements the resolver rejects
    /// and pairs the resolved line with the mount-time column.
    ///
    /// Case: a frame is emitted while one of two mounted webviews has
    /// scrolled out of the grid ring entirely.
    #[test]
    fn a_projection_omits_what_the_resolver_rejects() {
        let mut table = table();
        let grid = grid();
        mount(&mut table, &grid, 1);
        let projected = table.project(|_| Some(GridLine(-4)));
        assert_eq!(projected.len(), 1);
        assert_eq!(projected[0].id, InstanceId(1));
        assert_eq!(projected[0].point.line, GridLine(-4));
        assert_eq!(projected[0].point.column, GridColumn(0));
        assert!(table.project(|_| None).is_empty());
    }

    /// Asserts that the sweep drops exactly the placements the resolver
    /// rejects and names them.
    ///
    /// Case: a chunk of output scrolls one webview's anchor row out of
    /// the ring while another stays put.
    #[test]
    fn a_sweep_drops_and_names_the_unresolvable_placements() {
        let mut table = table();
        let grid = grid();
        mount(&mut table, &grid, 1);
        assert!(table.evict_lost_anchors(|_| Some(GridLine(0))).is_empty());
        assert_eq!(table.evict_lost_anchors(|_| None), vec![InstanceId(1)]);
        assert!(table.is_empty());
    }

    /// Asserts that an empty table resolves no anchor at all.
    ///
    /// Case: a chunk ends on a screen that has never held a webview, and
    /// the sweep runs anyway.
    #[test]
    fn an_empty_table_resolves_no_anchor() {
        let mut table = table();
        let mut resolved = 0;
        let swept = table.evict_lost_anchors(|_| {
            resolved += 1;
            None
        });
        assert!(swept.is_empty());
        assert_eq!(resolved, 0);
    }
}
