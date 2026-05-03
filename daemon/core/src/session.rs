use crate::{
    error::OzmuxResult,
    session::{
        cell::{Cell, CellId, LayoutCell, LayoutCellStore, PaneCell, SplitCell, SplitOrientation},
        pane::{Pane, PaneId, PaneStore},
    },
};

mod activity;
pub mod cell;
pub mod pane;

pub struct SessionStore {
    cells: LayoutCellStore,
    panes: PaneStore,
}

impl SessionStore {
    pub fn split_pane(
        &mut self,
        lhs_pane_id: &PaneId,
        rhs_pane_id: &PaneId,
        orientation: SplitOrientation,
    ) -> OzmuxResult {
        let lhs_pane = self.panes.get(lhs_pane_id)?;
        let rhs_pane = self.panes.get(&rhs_pane_id)?;
        let split_cell_id = CellId::new();
        let lhs_cell_id = lhs_pane.cell.clone();
        let rhs_cell_id = rhs_pane.cell.clone();
        let split_cell = SplitCell::new(lhs_cell_id, rhs_cell_id, orientation);

        Ok(())
    }
}

impl Default for SessionStore {
    fn default() -> Self {
        let pane_id = PaneId::new();
        let mut cells = LayoutCellStore::default();
        let cell_id = cells.create_pane_cell(pane_id.clone(), None);

        let mut panes = PaneStore::default();
        panes.insert(pane_id, Pane::new(cell_id));

        Self { panes, cells }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn default_pane_create() {}
}
