use crate::{
    define_string_new_type,
    error::{OzmuxError, OzmuxResult},
    session::cell::CellId,
};
use std::{collections::HashMap, fmt::Display};

#[derive(Clone, Debug, Default)]
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
}

#[derive(Clone, Debug)]
pub struct Pane {
    pub cell: CellId,
}

impl Pane {
    pub fn new(cell: CellId) -> Self {
        Self { cell }
    }
}

define_string_new_type!(PaneId);
