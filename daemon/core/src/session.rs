use crate::{
    error::OzmuxResult,
    session::{
        cell::{LayoutCellStore, Side, SplitOrientation},
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
    pub fn split_pane(&mut self, pane_id: &PaneId, orientation: SplitOrientation) -> OzmuxResult {
        let lhs_cell_id = self.panes.get(pane_id)?.cell.clone();
        let rhs_pane_id = PaneId::new();
        let rhs_cell_id = self.cells.create_pane_cell(rhs_pane_id.clone(), None);
        self.panes
            .insert(rhs_pane_id, Pane::new(rhs_cell_id.clone()));
        self.cells
            .split_cell(lhs_cell_id, rhs_cell_id, Side::After, orientation)?;

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
