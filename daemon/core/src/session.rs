use crate::{
    error::OzmuxResult,
    session::{
        cell::{CellId, LayoutCellStore, Side, SplitOrientation},
        pane::{Pane, PaneId, PaneStore},
    },
};

mod activity;
pub mod cell;
pub mod pane;

pub struct SessionStore {
    root: CellId,
    cells: LayoutCellStore,
    panes: PaneStore,
}

impl SessionStore {
    pub fn root(&self) -> &CellId {
        &self.root
    }

    pub fn split_pane(&mut self, pane_id: &PaneId, orientation: SplitOrientation) -> OzmuxResult {
        let target_cell_id = self.panes.get(pane_id)?.cell.clone();
        let target_was_root = target_cell_id == self.root;

        let new_pane_id = PaneId::new();
        let new_cell_id = self.cells.create_pane_cell(new_pane_id.clone(), None);
        self.panes
            .insert(new_pane_id, Pane::new(new_cell_id.clone()));

        let new_split_id =
            self.cells
                .split_cell(target_cell_id, new_cell_id, Side::After, orientation)?;

        if target_was_root {
            self.root = new_split_id;
        }

        Ok(())
    }
}

impl Default for SessionStore {
    fn default() -> Self {
        let pane_id = PaneId::new();
        let mut cells = LayoutCellStore::default();
        let cell_id = cells.create_pane_cell(pane_id.clone(), None);

        let mut panes = PaneStore::default();
        panes.insert(pane_id, Pane::new(cell_id.clone()));

        Self {
            root: cell_id,
            panes,
            cells,
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn default_pane_create() {}
}
