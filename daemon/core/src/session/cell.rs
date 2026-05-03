use crate::{
    define_string_new_type,
    error::{OzmuxError, OzmuxResult},
    session::pane::PaneId,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Default, Debug)]
pub struct LayoutCellStore(HashMap<CellId, LayoutCell>);

impl LayoutCellStore {
    #[inline]
    pub fn insert(&mut self, id: CellId, node: LayoutCell) {
        self.0.insert(id, node);
    }

    #[inline]
    pub fn parent(&self, id: &CellId) -> OzmuxResult<Option<&CellId>> {
        Ok(self.cell(id)?.parent.as_ref())
    }

    #[inline]
    pub fn pane_cell(&self, id: &CellId) -> OzmuxResult<&PaneCell> {
        match &self.cell(id)?.cell {
            Cell::Pane(pane) => Ok(pane),
            _ => Err(OzmuxError::InvalidNodeType(id.clone())),
        }
    }

    pub fn create_split_cell(
        &mut self,
        lhs: CellId,
        rhs: CellId,
        orientation: SplitOrientation,
    ) -> OzmuxResult<CellId> {
        let split_cell_id = CellId::new();
        let lhs_parent = self.parent(&lhs)?.cloned();
        let rhs_parent = self.parent(&rhs)?.cloned();
        self.cell_mut(&lhs)?.parent.replace(split_cell_id.clone());
        self.cell_mut(&rhs)?.parent.replace(split_cell_id.clone());

        self.replace_child_to_split_cell(&lhs, lhs_parent.clone(), split_cell_id.clone())?;
        self.replace_child_to_split_cell(&rhs, rhs_parent, split_cell_id.clone())?;

        self.0.insert(
            split_cell_id.clone(),
            LayoutCell {
                parent: lhs_parent,
                cell: Cell::Split(SplitCell::new(lhs, rhs, orientation)),
            },
        );
        Ok(split_cell_id)
    }

    #[inline]
    pub fn create_pane_cell(&mut self, pane_id: PaneId, parent: Option<CellId>) -> CellId {
        let id = CellId::new();
        self.insert(
            id.clone(),
            LayoutCell {
                parent,
                cell: Cell::Pane(PaneCell { pane: pane_id }),
            },
        );
        id
    }

    #[inline]
    pub fn cell(&self, id: &CellId) -> OzmuxResult<&LayoutCell> {
        self.0
            .get(id)
            .ok_or_else(|| OzmuxError::NodeNotfound(id.clone()))
    }

    #[inline]
    fn cell_mut(&mut self, id: &CellId) -> OzmuxResult<&mut LayoutCell> {
        self.0
            .get_mut(id)
            .ok_or_else(|| OzmuxError::NodeNotfound(id.clone()))
    }

    fn replace_child_to_split_cell(
        &mut self,
        cell: &CellId,
        parent: Option<CellId>,
        split_cell: CellId,
    ) -> OzmuxResult {
        let Some(parent) = parent.as_ref() else {
            return Ok(());
        };
        let Cell::Split(ref mut p) = self.cell_mut(parent)?.cell else {
            return Ok(());
        };
        if &p.lhs_cell == cell {
            p.lhs_cell = split_cell;
        } else if &p.rhs_cell == cell {
            p.rhs_cell = split_cell;
        }
        Ok(())
    }
}

define_string_new_type!(CellId);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayoutCell {
    pub parent: Option<CellId>,
    pub cell: Cell,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Cell {
    Pane(PaneCell),
    Split(SplitCell),
}

impl Cell {
    pub fn pane(pane: PaneId) -> Self {
        Self::Pane(PaneCell { pane })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaneCell {
    pub pane: PaneId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SplitCell {
    pub orientation: SplitOrientation,
    pub lhs_cell: CellId,
    pub lhs_weight: f32,
    pub rhs_cell: CellId,
    pub rhs_weight: f32,
}

impl SplitCell {
    pub fn new(lhs: CellId, rhs: CellId, orientation: SplitOrientation) -> Self {
        Self {
            orientation,
            lhs_cell: lhs,
            lhs_weight: 0.5,
            rhs_cell: rhs,
            rhs_weight: 0.5,
        }
    }
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub enum SplitOrientation {
    Vertical,
    Horizontal,
}
