//! The webview placements one screen owns: the mount table, the
//! resolution of its anchors into grid coordinates, and the eviction
//! sweep.

use crate::placement::{AnchoredPlacement, PlacementId, PlacementSize};
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
    /// [`Self::evict_lost_anchors`] returns on this before resolving a
    /// single anchor, which is what keeps sweeping a screen with no
    /// webview free.
    pub fn is_empty(&self) -> bool {
        self.placements.is_empty()
    }

    /// Registers a mount; the caller has already minted `id` and resolved
    /// the anchor.
    pub fn mount(
        &mut self,
        id: PlacementId,
        anchor: LineId,
        col: GridColumn,
        size: PlacementSize,
        view_id: String,
        instance_id: Option<String>,
    ) {
        self.placements.push(Placement {
            id,
            anchor,
            col,
            size,
            view_id,
            instance_id,
        });
    }

    /// Drops the placement a re-mount replaces, without reporting it.
    ///
    /// A superseded id is deliberately unnamed: the host re-points the
    /// same entity at the successor and would despawn it if the
    /// superseded id were reported as evicted.
    pub fn supersede(&mut self, view_id: &str, instance_id: Option<&str>) {
        self.placements
            .retain(|p| !p.addressed_by(view_id, instance_id));
    }

    /// Removes the placements a client `unmount` addresses; returns
    /// whether anything went.
    pub fn unmount(&mut self, view_id: Option<&str>, instance_id: Option<&str>) -> bool {
        let before = self.placements.len();
        self.placements.retain(|p| match (view_id, instance_id) {
            (None, _) => false,
            (Some(view), None) => p.view_id != view,
            (Some(view), Some(instance)) => !p.addressed_by(view, Some(instance)),
        });
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
    ) -> Vec<PlacementId> {
        if self.is_empty() {
            return Vec::new();
        }
        self.evict_where(|p| line_of(p.anchor).is_none())
    }

    /// Empties the table and names every id it held.
    pub fn take_all(&mut self) -> Vec<PlacementId> {
        self.evict_where(|_| true)
    }

    fn evict_where(
        &mut self,
        should_evict: impl FnMut(&mut Placement) -> bool,
    ) -> Vec<PlacementId> {
        self.placements
            .extract_if(.., should_evict)
            .map(|p| p.id)
            .collect()
    }
}

/// One mounted webview on this screen.
#[derive(Debug)]
struct Placement {
    id: PlacementId,
    anchor: LineId,
    col: GridColumn,
    size: PlacementSize,
    view_id: String,
    instance_id: Option<String>,
}

impl Placement {
    /// Whether this placement is the one `(view_id, instance_id)`
    /// addresses.
    fn addressed_by(&self, view_id: &str, instance_id: Option<&str>) -> bool {
        self.view_id == view_id && self.instance_id.as_deref() == instance_id
    }
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

    fn mount(table: &mut ScreenPlacements, grid: &Grid, id: u64, view: &str) {
        table.mount(
            PlacementId(id),
            grid.line_id(ScreenLine::TOP),
            GridColumn(0),
            PlacementSize { rows: 2, cols: 4 },
            view.to_string(),
            None,
        );
    }

    /// Asserts that a re-mount of the same address replaces the live
    /// placement instead of stacking a second one beside it.
    ///
    /// Case: a program re-renders the same named view after its content
    /// changed.
    #[test]
    fn a_supersede_replaces_the_live_placement_at_the_same_address() {
        let mut table = table();
        let grid = grid();
        mount(&mut table, &grid, 1, "memo");
        table.supersede("memo", None);
        mount(&mut table, &grid, 2, "memo");
        assert_eq!(table.len(), 1);
        assert_eq!(table.take_all(), vec![PlacementId(2)]);
    }

    /// Asserts that an unmount naming only a view id removes every
    /// placement at that view, an unmount naming an instance id too
    /// removes just that instance, and a `None` view id removes all.
    ///
    /// Case: a program tears down one of its views, then exits and asks
    /// the terminal to drop whatever is left.
    #[test]
    fn an_unmount_removes_the_placements_its_address_names() {
        let mut table = table();
        let grid = grid();
        mount(&mut table, &grid, 1, "memo");
        mount(&mut table, &grid, 2, "chart");
        assert!(table.unmount(Some("memo"), None));
        assert_eq!(table.len(), 1);
        assert!(!table.unmount(Some("memo"), None));
        assert!(table.unmount(None, None));
        assert!(table.is_empty());

        table.mount(
            PlacementId(3),
            grid.line_id(ScreenLine::TOP),
            GridColumn(0),
            PlacementSize { rows: 2, cols: 4 },
            "chart".to_string(),
            Some("a".to_string()),
        );
        assert!(!table.unmount(Some("chart"), Some("b")));
        assert!(table.unmount(Some("chart"), Some("a")));
        assert!(table.is_empty());
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
        mount(&mut table, &grid, 1, "memo");
        let projected = table.project(|_| Some(GridLine(-4)));
        assert_eq!(projected.len(), 1);
        assert_eq!(projected[0].id, PlacementId(1));
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
        mount(&mut table, &grid, 1, "memo");
        assert!(table.evict_lost_anchors(|_| Some(GridLine(0))).is_empty());
        assert_eq!(table.evict_lost_anchors(|_| None), vec![PlacementId(1)]);
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
