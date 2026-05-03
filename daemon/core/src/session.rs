use crate::{
    error::{OzmuxError, OzmuxResult},
    session::{
        cell::{CellId, CloseOutcome, LayoutCellStore, Side, SplitOrientation},
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

    /// Close a pane and propagate the structural change.
    ///
    /// Rejects closing the last pane (returns `CannotCloseLastPane`); closing the only
    /// pane equals ending the session, which is a separate API.
    ///
    /// On success: the pane is removed from `PaneStore`, the layout cell tree is
    /// updated via sibling-promote, and `self.root` is updated if the root changed.
    pub fn close_pane(&mut self, pane_id: &PaneId) -> OzmuxResult {
        let cell_id = self.panes.get(pane_id)?.cell.clone();

        // Pre-check: closing the only cell empties the tree, which equals "session ended".
        // The session-level invariant is "≥1 pane"; reject before mutating.
        if cell_id == self.root && self.panes.iter().count() == 1 {
            return Err(OzmuxError::CannotCloseLastPane(pane_id.clone()));
        }

        let outcome = self.cells.close_cell(&cell_id)?;
        match outcome {
            CloseOutcome::TreeEmptied => {
                // Defensive: should be unreachable given the pre-check above.
                return Err(OzmuxError::CannotCloseLastPane(pane_id.clone()));
            }
            CloseOutcome::RootReplaced { new_root } => {
                self.root = new_root;
            }
            CloseOutcome::SiblingPromoted { .. } => {
                // No root change.
            }
        }

        self.panes.remove(pane_id)?;
        Ok(())
    }
}

#[cfg(test)]
impl SessionStore {
    pub(crate) fn panes(&self) -> &PaneStore {
        &self.panes
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
    use super::*;
    use crate::error::OzmuxError;

    #[test]
    fn default_session_root_points_to_initial_pane() {
        let store = SessionStore::default();
        let root_id = store.root();
        let pane_id = store.panes().any_pane_id().expect("default has 1 pane");
        let pane = store.panes().get(&pane_id).expect("pane should exist");
        assert_eq!(&pane.cell, root_id);
    }

    #[test]
    fn close_pane_on_last_pane_returns_err() {
        let mut store = SessionStore::default();
        let last_pane_id = store.panes().any_pane_id().expect("default has 1 pane");

        let result = store.close_pane(&last_pane_id);
        assert!(matches!(
            result,
            Err(OzmuxError::CannotCloseLastPane(ref id)) if id == &last_pane_id
        ));
    }

    #[test]
    fn close_pane_after_split_removes_one_pane_and_updates_root() {
        let mut store = SessionStore::default();
        let first_pane_id = store.panes().any_pane_id().expect("default has 1 pane");
        let original_root = store.root().clone();

        store
            .split_pane(&first_pane_id, SplitOrientation::Horizontal)
            .expect("split");
        let root_after_split = store.root().clone();
        assert_ne!(
            root_after_split, original_root,
            "split of root must change root id"
        );

        store
            .close_pane(&first_pane_id)
            .expect("close_pane should succeed");

        assert!(
            store.panes().get(&first_pane_id).is_err(),
            "first_pane_id should be removed from PaneStore"
        );
        assert_ne!(
            store.root(),
            &root_after_split,
            "root should have changed (the root split is now collapsed)"
        );
    }
}
