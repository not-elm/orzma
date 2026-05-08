use crate::{
    error::{SessionError, SessionResult},
    pane::PaneId,
};
use ozmux_macros::NewType;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Default, Debug, Clone, Serialize)]
pub struct LayoutCellState(HashMap<CellId, Cell>);

impl LayoutCellState {
    #[inline]
    fn register(&mut self, id: CellId, node: Cell) {
        self.0.insert(id, node);
    }

    pub fn split_cell(
        &mut self,
        target: CellId,
        new_cell: CellId,
        new_cell_side: Side,
        orientation: SplitOrientation,
    ) -> SessionResult<CellId> {
        if target == new_cell {
            return Err(SessionError::SplitTargetEqualsNewCell(target));
        }
        let split_cell_id = CellId::new();
        let target_parent = self.parent(&target)?.cloned();
        self.cell(&new_cell)?;
        self.cell_mut(&target)?
            .parent
            .replace(split_cell_id.clone());
        self.cell_mut(&new_cell)?
            .parent
            .replace(split_cell_id.clone());

        self.repoint_parent_to_split(&target, target_parent.clone(), split_cell_id.clone())?;

        let (lhs_cell, rhs_cell) = match new_cell_side {
            Side::Before => (new_cell, target),
            Side::After => (target, new_cell),
        };

        self.0.insert(
            split_cell_id.clone(),
            LayoutCell {
                parent: target_parent,
                cell: Cell::Split(SplitCell::new(lhs_cell, rhs_cell, orientation)),
            },
        );
        Ok(split_cell_id)
    }

    /// Close a leaf cell using sibling-promote semantics.
    ///
    /// Reads as: validate the target is a closable leaf, plan the collapse from
    /// pure reads, then atomically apply it. The `&self → CollapsePlan → &mut self`
    /// flow encodes pre-validate-then-commit atomicity in the type signature: the
    /// planning phase cannot mutate, so any `Err` it returns leaves the store
    /// logically unchanged (no keys added, removed, or reassigned).
    ///
    /// # Identifier stability
    /// Only `id` (and the parent split, if any) are removed from the store.
    /// Every other `CellId` continues to resolve to the same cell; only the
    /// promoted sibling's `parent` field and the grandparent's child slot are updated.
    pub fn close_cell(&mut self, id: &CellId) -> SessionResult<CloseOutcome> {
        let target = self.cell(id)?;
        if !matches!(target.cell, Cell::Pane(_)) {
            return Err(SessionError::InvalidCellType(id.clone()));
        }

        let Some(parent_id) = target.parent.clone() else {
            // Target has no parent — it IS the root pane. Removing it empties the tree.
            self.0.remove(id);
            return Ok(CloseOutcome::TreeEmptied);
        };

        let plan = self.plan_collapse(id, parent_id)?;
        Ok(self.apply_collapse(id, plan))
    }

    /// Read-only validation: gather everything `apply_collapse` needs, without mutating.
    ///
    /// Taking `&self` makes it a compile-time guarantee that this phase performs no
    /// writes — any `Err` propagated from here leaves the store unchanged.
    fn plan_collapse(&self, target_id: &CellId, parent_id: CellId) -> SessionResult<CollapsePlan> {
        let parent = self.cell(&parent_id)?;
        let Cell::Split(parent_split) = &parent.cell else {
            return Err(SessionError::InvalidCellType(parent_id));
        };
        let sibling_id = parent_split.sibling_cell_id(target_id).clone();
        let grandparent_id = parent.parent.clone();

        if let Some(gp_id) = grandparent_id.as_ref() {
            let grandparent = self.cell(gp_id)?;
            if !matches!(grandparent.cell, Cell::Split(_)) {
                return Err(SessionError::InvalidCellType(gp_id.clone()));
            }
        }

        Ok(CollapsePlan {
            parent_id,
            sibling_id,
            grandparent_id,
        })
    }

    /// Atomically apply a planned collapse. Infallible by construction — every lookup
    /// here was already verified during planning, so `expect`/`unreachable!` are safe.
    fn apply_collapse(&mut self, target_id: &CellId, plan: CollapsePlan) -> CloseOutcome {
        self.0.remove(target_id);
        self.0
            .remove(&plan.parent_id)
            .expect("parent existed in plan");
        let sibling = self
            .0
            .get_mut(&plan.sibling_id)
            .expect("sibling existed in plan");
        sibling.parent = plan.grandparent_id.clone();

        match plan.grandparent_id {
            Some(gp_id) => {
                self.rewire_grandparent_child(&gp_id, &plan.parent_id, &plan.sibling_id);
                CloseOutcome::SiblingPromoted {
                    survivor: plan.sibling_id,
                    new_parent: gp_id,
                }
            }
            None => CloseOutcome::RootReplaced {
                new_root: plan.sibling_id,
            },
        }
    }

    /// Replace `old_child` with `new_child` in `grandparent_id`'s split, preserving
    /// the lhs/rhs slot orientation (slot pinning).
    fn rewire_grandparent_child(
        &mut self,
        grandparent_id: &CellId,
        old_child: &CellId,
        new_child: &CellId,
    ) {
        let grandparent = self
            .0
            .get_mut(grandparent_id)
            .expect("grandparent existed in plan");
        let Cell::Split(gp_split) = &mut grandparent.cell else {
            unreachable!("grandparent type checked in plan");
        };
        if &gp_split.lhs_cell == old_child {
            gp_split.lhs_cell = new_child.clone();
        } else if &gp_split.rhs_cell == old_child {
            gp_split.rhs_cell = new_child.clone();
        } else {
            unreachable!(
                "bidirectional parent/child invariant: grandparent must reference old_child"
            );
        }
    }

    pub fn new_root(&mut self) -> CellId {
        let id = CellId::new();
        self.0.insert(id.clone(), Cell::Root);
        id
    }

    #[inline]
    pub fn new_pane(&mut self, pane_id: PaneId, parent: CellId) -> CellId {
        let id = CellId::new();
        self.register(
            id.clone(),
            LayoutCell {
                parent,
                cell: Cell::Pane(PaneCell { pane: pane_id }),
            },
        );
        id
    }

    #[inline]
    pub fn cell(&self, id: &CellId) -> SessionResult<&LayoutCell> {
        self.0
            .get(id)
            .ok_or_else(|| SessionError::CellNotFound(id.clone()))
    }

    #[inline]
    fn cell_mut(&mut self, id: &CellId) -> SessionResult<&mut LayoutCell> {
        self.0
            .get_mut(id)
            .ok_or_else(|| SessionError::CellNotFound(id.clone()))
    }

    fn repoint_parent_to_split(
        &mut self,
        cell: &CellId,
        parent: Option<CellId>,
        split_cell: CellId,
    ) -> SessionResult {
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

    #[inline]
    fn parent(&self, id: &CellId) -> SessionResult<Option<&CellId>> {
        Ok(self.cell(id)?.parent.as_ref())
    }
}

/// Internal record gathered by `LayoutCellState::plan_collapse` and consumed by
/// `apply_collapse`. Constructed only inside `cell.rs`, so its existence implies
/// every reference inside has been verified against the store.
#[derive(Debug)]
struct CollapsePlan {
    parent_id: CellId,
    sibling_id: CellId,
    grandparent_id: Option<CellId>,
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize, NewType)]
#[newtype(as_ref(str), display, new(uuid_v4_string), default)]
pub struct CellId(String);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Cell {
    Root,
    Pane(PaneCell),
    Split(SplitCell),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaneCell {
    pub parent: CellId,
    pub pane: PaneId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SplitCell {
    pub parent: CellId,
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

    pub fn sibling_cell_id(&self, id: &CellId) -> &CellId {
        if &self.lhs_cell == id {
            &self.rhs_cell
        } else {
            &self.lhs_cell
        }
    }
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SplitOrientation {
    Vertical,
    Horizontal,
}

#[derive(Debug, Default, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    /// Place new_cell before target (left or top, depending on orientation).
    Before,
    /// Place new_cell after target (right or bottom, depending on orientation).
    #[default]
    After,
}

/// Structural outcome of `LayoutCellState::close_cell`.
///
/// Callers (typically `Session::close_pane`) must handle every variant.
/// `#[must_use]` is a lint-level nudge; type-level enforcement comes from
/// requiring the caller to consume the value via `match`.
#[must_use]
#[derive(Debug, Clone, PartialEq)]
pub enum CloseOutcome {
    /// Target had no parent; the store is now empty.
    /// Callers should treat this as "session ended" — closing the only pane
    /// equals tearing down the layout entirely.
    TreeEmptied,

    /// Target's parent split was the root; the surviving sibling becomes
    /// the new root (its `parent` field is now `None`).
    RootReplaced { new_root: CellId },

    /// Target's grandparent existed; the surviving sibling now occupies
    /// `new_parent`'s child slot in the same lhs/rhs position the deleted
    /// parent occupied (slot pinning).
    SiblingPromoted {
        survivor: CellId,
        new_parent: CellId,
    },
}
