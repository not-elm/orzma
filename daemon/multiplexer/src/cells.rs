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
    /// Register a pane cell. `parent == None` makes the new cell the tree's
    /// root — the caller (typically `Window`) is responsible for storing the
    /// returned `CellId` as `Window.root_cell`.
    pub fn new_pane(&mut self, pane_id: PaneId, parent: Option<CellId>) -> CellId {
        let id = CellId::new();
        self.0.insert(
            id.clone(),
            Cell::Pane(PaneCell {
                parent,
                pane: pane_id,
            }),
        );
        id
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
        let target_parent = self.cell(&target)?.parent().cloned();
        self.cell(&new_cell)?;

        let split_id = CellId::new();
        self.reparent(&target, Some(split_id.clone()))?;
        self.reparent(&new_cell, Some(split_id.clone()))?;

        // If target had a parent split, swap the target slot to the new split.
        // If target was the tree root (parent == None), there is no upward link
        // to rewire here — the caller must update `Window.root_cell` to
        // `split_id` to keep the window pointing at the new top.
        if let Some(parent_id) = target_parent.as_ref() {
            self.cell_mut(parent_id)?
                .replace_child(&target, split_id.clone());
        }

        let (lhs_cell, rhs_cell) = match new_cell_side {
            Side::Before => (new_cell, target),
            Side::After => (target, new_cell),
        };
        self.0.insert(
            split_id.clone(),
            Cell::Split(SplitCell::new(target_parent, lhs_cell, rhs_cell, orientation)),
        );
        Ok(split_id)
    }

    /// Close a leaf cell using sibling-promote semantics.
    ///
    /// Reads as: validate the target is a closable leaf, plan the collapse from
    /// pure reads, then atomically apply it. The `&self → CollapsePlan → &mut self`
    /// flow encodes pre-validate-then-commit atomicity in the type signature: the
    /// planning phase cannot mutate, so any `Err` it returns leaves the store
    /// logically unchanged.
    pub fn close_cell(&mut self, id: &CellId) -> SessionResult<CloseOutcome> {
        let Some(parent_id) = self.target_pane_parent(id)? else {
            // Target was the lone root pane — removing it empties the tree.
            // Window layer interprets this as "tear down the window".
            self.0.remove(id);
            return Ok(CloseOutcome::TreeEmptied);
        };
        let plan = self.plan_collapse(id, parent_id)?;
        Ok(self.apply_collapse(id, plan))
    }

    fn target_pane_parent(&self, id: &CellId) -> SessionResult<Option<CellId>> {
        match self.cell(id)? {
            Cell::Pane(p) => Ok(p.parent.clone()),
            Cell::Split(_) => Err(SessionError::InvalidCellType(id.clone())),
        }
    }

    /// Read-only validation: gather everything `apply_collapse` needs, without
    /// mutating. Taking `&self` makes it a compile-time guarantee that this
    /// phase performs no writes — any `Err` propagated from here leaves the
    /// store unchanged.
    fn plan_collapse(&self, target_id: &CellId, parent_id: CellId) -> SessionResult<CollapsePlan> {
        let parent_split = match self.cell(&parent_id)? {
            Cell::Split(s) => s,
            Cell::Pane(_) => return Err(SessionError::InvalidCellType(parent_id)),
        };
        let sibling_id = parent_split.sibling_cell_id(target_id).clone();
        let grandparent_id = parent_split.parent.clone();

        if let Some(gp_id) = grandparent_id.as_ref() {
            if matches!(self.cell(gp_id)?, Cell::Pane(_)) {
                return Err(SessionError::InvalidCellType(gp_id.clone()));
            }
        }
        Ok(CollapsePlan {
            parent_id,
            sibling_id,
            grandparent_id,
        })
    }

    /// Atomically apply a planned collapse. Infallible by construction — every
    /// lookup here was verified during planning, so `expect` is safe.
    fn apply_collapse(&mut self, target_id: &CellId, plan: CollapsePlan) -> CloseOutcome {
        self.0.remove(target_id);
        self.0
            .remove(&plan.parent_id)
            .expect("parent existed in plan");
        self.reparent(&plan.sibling_id, plan.grandparent_id.clone())
            .expect("sibling existed in plan");

        match plan.grandparent_id {
            Some(gp_id) => {
                self.cell_mut(&gp_id)
                    .expect("grandparent existed in plan")
                    .replace_child(&plan.parent_id, plan.sibling_id.clone());
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

    fn reparent(&mut self, child: &CellId, new_parent: Option<CellId>) -> SessionResult {
        self.cell_mut(child)?.set_parent(new_parent);
        Ok(())
    }

    #[inline]
    pub fn cell(&self, id: &CellId) -> SessionResult<&Cell> {
        self.0
            .get(id)
            .ok_or_else(|| SessionError::CellNotFound(id.clone()))
    }

    #[inline]
    fn cell_mut(&mut self, id: &CellId) -> SessionResult<&mut Cell> {
        self.0
            .get_mut(id)
            .ok_or_else(|| SessionError::CellNotFound(id.clone()))
    }
}

/// Internal record gathered by `plan_collapse` and consumed by `apply_collapse`.
/// Constructed only inside this module, so its existence implies every reference
/// inside has been verified against the store.
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
    Pane(PaneCell),
    Split(SplitCell),
}

impl Cell {
    pub fn parent(&self) -> Option<&CellId> {
        match self {
            Self::Pane(c) => c.parent.as_ref(),
            Self::Split(c) => c.parent.as_ref(),
        }
    }

    fn set_parent(&mut self, parent: Option<CellId>) {
        match self {
            Self::Pane(c) => c.parent = parent,
            Self::Split(c) => c.parent = parent,
        }
    }

    /// Replace a downward child reference. Only `Split` carries children;
    /// calling this on a `Pane` is an invariant violation.
    fn replace_child(&mut self, old: &CellId, new: CellId) {
        match self {
            Self::Split(s) => {
                if &s.lhs_cell == old {
                    s.lhs_cell = new;
                } else if &s.rhs_cell == old {
                    s.rhs_cell = new;
                } else {
                    unreachable!("split does not reference {old} as a child");
                }
            }
            Self::Pane(_) => unreachable!("Cell::Pane has no children"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaneCell {
    pub parent: Option<CellId>,
    pub pane: PaneId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SplitCell {
    pub parent: Option<CellId>,
    pub orientation: SplitOrientation,
    pub lhs_cell: CellId,
    pub lhs_weight: f32,
    pub rhs_cell: CellId,
    pub rhs_weight: f32,
}

impl SplitCell {
    pub fn new(
        parent: Option<CellId>,
        lhs: CellId,
        rhs: CellId,
        orientation: SplitOrientation,
    ) -> Self {
        Self {
            parent,
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
/// Callers (typically `Window::close_pane`) must handle every variant.
/// `#[must_use]` is a lint-level nudge; type-level enforcement comes from
/// requiring the caller to consume the value via `match`.
#[must_use]
#[derive(Debug, Clone, PartialEq)]
pub enum CloseOutcome {
    /// Target was the lone root pane. The store is now empty for this tree;
    /// the caller should treat this as "tear down the window".
    TreeEmptied,
    /// Target's parent split was the tree root. `new_root` (the surviving
    /// sibling) now has `parent == None` and the caller must update
    /// `Window.root_cell` to point at it.
    RootReplaced { new_root: CellId },
    /// Target's grandparent was a split; survivor occupies the same lhs/rhs
    /// slot of `new_parent` that the deleted parent occupied (slot pinning).
    /// `Window.root_cell` is unchanged.
    SiblingPromoted {
        survivor: CellId,
        new_parent: CellId,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pid() -> PaneId {
        PaneId::new()
    }

    #[test]
    fn new_pane_with_no_parent_is_root() {
        let mut state = LayoutCellState::default();
        let pane_id = pid();
        let cell_id = state.new_pane(pane_id.clone(), None);

        let Cell::Pane(pane) = state.cell(&cell_id).unwrap() else {
            panic!("expected Pane");
        };
        assert!(pane.parent.is_none());
        assert_eq!(pane.pane, pane_id);
    }

    #[test]
    fn split_cell_on_root_pane_promotes_split_to_root() {
        let mut state = LayoutCellState::default();
        let pane_a = state.new_pane(pid(), None);
        let pane_b = state.new_pane(pid(), Some(pane_a.clone()));

        let split_id = state
            .split_cell(
                pane_a.clone(),
                pane_b.clone(),
                Side::After,
                SplitOrientation::Horizontal,
            )
            .unwrap();

        let Cell::Split(split) = state.cell(&split_id).unwrap() else {
            panic!()
        };
        assert!(split.parent.is_none());
        assert_eq!(split.lhs_cell, pane_a);
        assert_eq!(split.rhs_cell, pane_b);

        let Cell::Pane(a) = state.cell(&pane_a).unwrap() else {
            panic!()
        };
        assert_eq!(a.parent.as_ref(), Some(&split_id));
        let Cell::Pane(b) = state.cell(&pane_b).unwrap() else {
            panic!()
        };
        assert_eq!(b.parent.as_ref(), Some(&split_id));
    }

    #[test]
    fn split_cell_on_non_root_pane_rewires_parent_split_slot() {
        let mut state = LayoutCellState::default();
        let pane_a = state.new_pane(pid(), None);
        let pane_b = state.new_pane(pid(), Some(pane_a.clone()));
        let outer = state
            .split_cell(
                pane_a.clone(),
                pane_b.clone(),
                Side::After,
                SplitOrientation::Horizontal,
            )
            .unwrap();

        let pane_c = state.new_pane(pid(), Some(outer.clone()));
        let inner = state
            .split_cell(
                pane_b.clone(),
                pane_c.clone(),
                Side::After,
                SplitOrientation::Vertical,
            )
            .unwrap();

        let Cell::Split(outer_split) = state.cell(&outer).unwrap() else {
            panic!()
        };
        assert_eq!(outer_split.lhs_cell, pane_a);
        assert_eq!(outer_split.rhs_cell, inner);
        let Cell::Split(inner_split) = state.cell(&inner).unwrap() else {
            panic!()
        };
        assert_eq!(inner_split.parent.as_ref(), Some(&outer));
        assert_eq!(inner_split.lhs_cell, pane_b);
        assert_eq!(inner_split.rhs_cell, pane_c);
    }

    #[test]
    fn close_cell_on_lone_root_pane_empties_tree() {
        let mut state = LayoutCellState::default();
        let pane = state.new_pane(pid(), None);

        let outcome = state.close_cell(&pane).unwrap();
        assert_eq!(outcome, CloseOutcome::TreeEmptied);
        assert!(state.cell(&pane).is_err());
    }

    #[test]
    fn close_cell_under_root_split_promotes_sibling_to_new_root() {
        let mut state = LayoutCellState::default();
        let pane_a = state.new_pane(pid(), None);
        let pane_b = state.new_pane(pid(), Some(pane_a.clone()));
        let split_id = state
            .split_cell(
                pane_a.clone(),
                pane_b.clone(),
                Side::After,
                SplitOrientation::Horizontal,
            )
            .unwrap();

        let outcome = state.close_cell(&pane_a).unwrap();
        assert_eq!(
            outcome,
            CloseOutcome::RootReplaced {
                new_root: pane_b.clone()
            }
        );

        let Cell::Pane(survivor) = state.cell(&pane_b).unwrap() else {
            panic!()
        };
        assert!(survivor.parent.is_none());
        assert!(state.cell(&split_id).is_err());
    }

    #[test]
    fn close_cell_under_nested_split_promotes_sibling_in_grandparent_slot() {
        let mut state = LayoutCellState::default();
        let pane_a = state.new_pane(pid(), None);
        let pane_b = state.new_pane(pid(), Some(pane_a.clone()));
        let outer = state
            .split_cell(
                pane_a.clone(),
                pane_b.clone(),
                Side::After,
                SplitOrientation::Horizontal,
            )
            .unwrap();
        let pane_c = state.new_pane(pid(), Some(outer.clone()));
        let inner = state
            .split_cell(
                pane_b.clone(),
                pane_c.clone(),
                Side::After,
                SplitOrientation::Vertical,
            )
            .unwrap();

        let outcome = state.close_cell(&pane_b).unwrap();
        assert_eq!(
            outcome,
            CloseOutcome::SiblingPromoted {
                survivor: pane_c.clone(),
                new_parent: outer.clone(),
            }
        );

        let Cell::Split(outer_split) = state.cell(&outer).unwrap() else {
            panic!()
        };
        assert_eq!(outer_split.lhs_cell, pane_a);
        assert_eq!(outer_split.rhs_cell, pane_c);
        let Cell::Pane(survivor) = state.cell(&pane_c).unwrap() else {
            panic!()
        };
        assert_eq!(survivor.parent.as_ref(), Some(&outer));
        assert!(state.cell(&inner).is_err());
    }
}
