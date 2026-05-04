use crate::{
    define_string_new_type,
    error::{OzmuxError, OzmuxResult},
    session::{activity::Activity, cell::CellId},
};
use serde::Serialize;
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
            .ok_or_else(|| OzmuxError::PaneNotFound(id.clone()))
    }

    #[inline]
    pub fn remove(&mut self, id: &PaneId) -> OzmuxResult<Pane> {
        self.0
            .remove(id)
            .ok_or_else(|| OzmuxError::PaneNotFound(id.clone()))
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[cfg(test)]
    #[inline]
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&PaneId, &Pane)> {
        self.0.iter()
    }

    #[cfg(test)]
    pub(crate) fn any_pane_id(&self) -> Option<PaneId> {
        self.0.keys().next().cloned()
    }
}

impl Serialize for PaneStore {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.0.values())
    }
}

#[derive(Debug, Serialize)]
pub struct Pane {
    id: PaneId,
    cell: CellId,
    activities: Vec<Activity>,
}

impl Pane {
    pub fn new(id: PaneId, cell: CellId) -> Self {
        let activities = vec![Activity::default()];
        Self {
            id,
            cell,
            activities,
        }
    }

    #[cfg(test)]
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
        assert!(matches!(result, Err(OzmuxError::PaneNotFound(ref err_id)) if err_id == &id));
    }

    #[test]
    fn pane_serializes_with_id_cell_activities() {
        let id = PaneId::new();
        let cell_id = CellId::new();
        let pane = Pane::new(id.clone(), cell_id.clone());
        let v = serde_json::to_value(&pane).unwrap();
        assert_eq!(v["id"].as_str(), Some(id.as_ref()));
        assert_eq!(v["cell"].as_str(), Some(cell_id.as_ref()));
        assert!(v["activities"].is_array());
    }

    #[test]
    fn pane_store_serializes_as_array_of_panes() {
        let mut store = PaneStore::default();
        let id1 = PaneId::new();
        let id2 = PaneId::new();
        store.insert(id1.clone(), Pane::new(id1.clone(), CellId::new()));
        store.insert(id2.clone(), Pane::new(id2.clone(), CellId::new()));
        let v = serde_json::to_value(&store).unwrap();
        let arr = v.as_array().expect("array");
        assert_eq!(arr.len(), 2);
        let ids: std::collections::HashSet<_> = arr
            .iter()
            .map(|item| item["id"].as_str().unwrap().to_string())
            .collect();
        assert!(ids.contains(id1.as_ref()));
        assert!(ids.contains(id2.as_ref()));
    }
}
