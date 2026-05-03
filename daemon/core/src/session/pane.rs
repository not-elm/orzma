use crate::{
    define_string_new_type,
    error::{OzmuxError, OzmuxResult},
    session::{activity::Activity, cell::CellId},
};
use std::collections::HashMap;

#[derive(Debug, Default)]
pub struct PaneStore(HashMap<PaneId, Pane>);

impl PaneStore {
    #[inline]
    pub fn insert(&mut self, id: PaneId, pane: Pane) {
        self.0.insert(id, pane);
    }

    #[inline]
    pub fn get(&self, id: &PaneId) -> OzmuxResult<&Pane> {
        self.0
            .get(id)
            .ok_or_else(|| OzmuxError::PaneNotfound(id.clone()))
    }

    #[inline]
    pub fn remove(&mut self, id: &PaneId) -> OzmuxResult<Pane> {
        self.0
            .remove(id)
            .ok_or_else(|| OzmuxError::PaneNotfound(id.clone()))
    }

    #[inline]
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&PaneId, &Pane)> {
        self.0.iter()
    }

    #[cfg(test)]
    pub(crate) fn any_pane_id(&self) -> Option<PaneId> {
        self.0.keys().next().cloned()
    }
}

#[derive(Debug)]
pub struct Pane {
    id: PaneId,
    cell: CellId,
    activities: Vec<Activity>,
}

impl Pane {
    pub fn new(id: PaneId, cell: CellId) -> Self {
        let activities = vec![Activity::default()];
        Self { id, cell, activities }
    }

    pub const fn id(&self) -> &PaneId {
        &self.id
    }

    pub const fn cell_id(&self) -> &CellId {
        &self.cell
    }
}

define_string_new_type!(PaneId);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::cell::CellId;

    #[test]
    fn remove_existing_pane_returns_pane() {
        let mut store = PaneStore::default();
        let id = PaneId::new();
        let cell_id = CellId::new();
        store.insert(id.clone(), Pane::new(id.clone(), cell_id.clone()));

        let removed = store.remove(&id).expect("remove should succeed");
        assert_eq!(removed.cell, cell_id);
        assert!(
            store.get(&id).is_err(),
            "pane should no longer be retrievable after remove"
        );
    }

    #[test]
    fn pane_carries_its_id() {
        let id = PaneId::new();
        let cell_id = CellId::new();
        let pane = Pane::new(id.clone(), cell_id.clone());
        assert_eq!(pane.id(), &id);
        assert_eq!(pane.cell_id(), &cell_id);
    }

    #[test]
    fn remove_nonexistent_pane_returns_err() {
        let mut store = PaneStore::default();
        let id = PaneId::new();
        let result = store.remove(&id);
        assert!(matches!(result, Err(OzmuxError::PaneNotfound(ref err_id)) if err_id == &id));
    }
}
