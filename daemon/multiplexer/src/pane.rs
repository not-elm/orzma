use crate::{
    activity::{Activity, ActivityId},
    cell::CellId,
    error::{SessionError, SessionResult},
};
use ozmux_macros::NewType;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize, NewType)]
#[newtype(as_ref(str), display, new(uuid_v4_string), default)]
pub struct PaneId(String);

#[derive(Debug, Default, Clone)]
pub struct PaneStore(HashMap<PaneId, Pane>);

impl PaneStore {
    #[inline]
    pub fn insert(&mut self, id: PaneId, pane: Pane) {
        self.0.insert(id, pane);
    }

    #[inline]
    pub fn get(&self, id: &PaneId) -> SessionResult<&Pane> {
        self.0
            .get(id)
            .ok_or_else(|| SessionError::PaneNotFound(id.clone()))
    }

    #[inline]
    pub fn remove(&mut self, id: &PaneId) -> SessionResult<Pane> {
        self.0
            .remove(id)
            .ok_or_else(|| SessionError::PaneNotFound(id.clone()))
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = (&PaneId, &Pane)> {
        self.0.iter()
    }

    pub fn any_pane_id(&self) -> Option<PaneId> {
        self.0.keys().next().cloned()
    }
}

impl Serialize for PaneStore {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut panes: Vec<&Pane> = self.0.values().collect();
        panes.sort_by(|a, b| a.id.as_ref().cmp(b.id.as_ref()));
        serializer.collect_seq(panes)
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Pane {
    pub id: PaneId,
    pub activities: Vec<ActivityId>,
}

impl Pane {
    pub fn new(id: PaneId) -> Self {
        let activities = vec![Activity::default()];
        Self { id, activities }
    }
}
