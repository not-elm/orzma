use crate::{
    error::{MultiplexerError, MultiplexerResult},
    window::pane::PaneId,
};
use ozmux_macros::NewType;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Axis-aligned rectangle in normalized window coordinates (`x, y, w, h` ∈ [0, 1]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct LayoutCellState(HashMap<CellId, Cell>);

impl LayoutCellState {
    /// Initialize a new window's layout: a `Cell::Root` and its single initial
    /// `Cell::Pane`, registered atomically so `RootCell::child` is always valid.
    /// `Window.root_cell` is set to the returned root id and stays invariant
    /// across subsequent splits / closes.
    pub fn new_window_layout(&mut self, pane_id: PaneId) -> (CellId, CellId) {
        let root_id = CellId::new();
        let pane_cell_id = CellId::new();
        self.0.insert(
            pane_cell_id.clone(),
            Cell::Pane(PaneCell {
                parent: Some(root_id.clone()),
                pane: pane_id,
            }),
        );
        self.0.insert(
            root_id.clone(),
            Cell::Root(RootCell {
                child: pane_cell_id.clone(),
            }),
        );
        (root_id, pane_cell_id)
    }

    /// Register a pane cell. `parent == None` produces an orphan that callers
    /// (typically `split_cell`) are expected to attach into the tree shortly
    /// after creation.
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
    ) -> MultiplexerResult<CellId> {
        if target == new_cell {
            return Err(MultiplexerError::SplitTargetEqualsNewCell(target));
        }
        let target_parent = self.cell(&target)?.require_parent()?;
        self.cell(&new_cell)?;

        let split_id = CellId::new();
        self.reparent(&target, Some(split_id.clone()))?;
        self.reparent(&new_cell, Some(split_id.clone()))?;
        self.cell_mut(&target_parent)?
            .replace_child(&target, split_id.clone());

        let (lhs_cell, rhs_cell) = match new_cell_side {
            Side::Before => (new_cell, target),
            Side::After => (target, new_cell),
        };
        self.0.insert(
            split_id.clone(),
            Cell::Split(SplitCell::new(
                Some(target_parent),
                lhs_cell,
                rhs_cell,
                orientation,
            )),
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
    pub fn close_cell(&mut self, id: &CellId) -> MultiplexerResult<CloseOutcome> {
        let parent_id = self.target_pane_parent(id)?;
        let plan = self.plan_collapse(id, parent_id)?;
        Ok(self.apply_collapse(id, plan))
    }

    fn target_pane_parent(&self, id: &CellId) -> MultiplexerResult<CellId> {
        match self.cell(id)? {
            Cell::Pane(p) => p.parent.clone().ok_or(MultiplexerError::MissingParentCell),
            _ => Err(MultiplexerError::InvalidCellType(id.clone())),
        }
    }

    /// Read-only validation: gather everything `apply_collapse` needs, without
    /// mutating. Taking `&self` makes it a compile-time guarantee that this
    /// phase performs no writes.
    fn plan_collapse(
        &self,
        target_id: &CellId,
        parent_id: CellId,
    ) -> MultiplexerResult<CollapsePlan> {
        let parent_split = match self.cell(&parent_id)? {
            Cell::Split(s) => s,
            // Pane is the only child of Root — closing it would empty the
            // window's layout, which the model forbids.
            Cell::Root(_) => return Err(MultiplexerError::CannotCloseLastPane(target_id.clone())),
            Cell::Pane(_) => return Err(MultiplexerError::InvalidCellType(parent_id)),
        };
        let sibling_id = parent_split.sibling_cell_id(target_id).clone();
        let grandparent_id = parent_split
            .parent
            .clone()
            .ok_or(MultiplexerError::MissingParentCell)?;
        match self.cell(&grandparent_id)? {
            Cell::Root(_) | Cell::Split(_) => {}
            Cell::Pane(_) => return Err(MultiplexerError::InvalidCellType(grandparent_id)),
        }
        Ok(CollapsePlan {
            parent_id,
            sibling_id,
            grandparent_id,
        })
    }

    fn apply_collapse(&mut self, target_id: &CellId, plan: CollapsePlan) -> CloseOutcome {
        self.0.remove(target_id);
        self.0
            .remove(&plan.parent_id)
            .expect("parent existed in plan");
        self.reparent(&plan.sibling_id, Some(plan.grandparent_id.clone()))
            .expect("sibling existed in plan");

        let grandparent = self
            .0
            .get_mut(&plan.grandparent_id)
            .expect("grandparent existed in plan");
        let promoted_to_root = matches!(grandparent, Cell::Root(_));
        grandparent.replace_child(&plan.parent_id, plan.sibling_id.clone());

        if promoted_to_root {
            CloseOutcome::PromotedToRootChild {
                survivor: plan.sibling_id,
                root: plan.grandparent_id,
            }
        } else {
            CloseOutcome::SiblingPromoted {
                survivor: plan.sibling_id,
                new_parent: plan.grandparent_id,
            }
        }
    }

    fn reparent(&mut self, child: &CellId, new_parent: Option<CellId>) -> MultiplexerResult {
        self.cell_mut(child)?.set_parent(new_parent);
        Ok(())
    }

    /// Walk down `start`'s subtree along the lhs/child branch and return the
    /// first `Cell::Pane` reached. Used to pick a representative pane id when
    /// promoting a survivor subtree to be active.
    pub fn leftmost_pane(&self, start: &CellId) -> MultiplexerResult<&PaneId> {
        let mut current: &CellId = start;
        loop {
            match self.cell(current)? {
                Cell::Pane(c) => return Ok(&c.pane),
                Cell::Split(s) => current = &s.lhs_cell,
                Cell::Root(r) => current = &r.child,
            }
        }
    }

    /// Collect every `PaneId` reachable from `start`'s subtree (Root or Split
    /// roots are descended; Pane leaves contribute their `PaneId`).
    pub fn pane_ids_in_subtree(&self, start: &CellId) -> MultiplexerResult<Vec<PaneId>> {
        let mut out = Vec::new();
        self.collect_panes(start, &mut out)?;
        Ok(out)
    }

    /// Same traversal as `pane_ids_in_subtree` but also yields each leaf's
    /// `CellId`. Used by `Window::swap_pane` to address two cells for the
    /// pane-field swap.
    pub(crate) fn ordered_pane_cells(
        &self,
        start: &CellId,
    ) -> MultiplexerResult<Vec<(CellId, PaneId)>> {
        let mut out = Vec::new();
        self.collect_pane_cells(start, &mut out)?;
        Ok(out)
    }

    fn collect_pane_cells(
        &self,
        id: &CellId,
        out: &mut Vec<(CellId, PaneId)>,
    ) -> MultiplexerResult {
        match self.cell(id)? {
            Cell::Root(r) => {
                let child = r.child.clone();
                self.collect_pane_cells(&child, out)?;
            }
            Cell::Split(s) => {
                let lhs = s.lhs_cell.clone();
                let rhs = s.rhs_cell.clone();
                self.collect_pane_cells(&lhs, out)?;
                self.collect_pane_cells(&rhs, out)?;
            }
            Cell::Pane(p) => out.push((id.clone(), p.pane.clone())),
        }
        Ok(())
    }

    /// Swap the `pane:` field of two `PaneCell` leaves. Errors with
    /// `InvalidCellType` if either id resolves to `Cell::Root` or
    /// `Cell::Split`. Cell ids, parent pointers, splits, and weights are
    /// untouched — only the pane payload of each cell moves.
    pub(crate) fn swap_panes(&mut self, a: &CellId, b: &CellId) -> MultiplexerResult {
        let pane_a = match self.cell(a)? {
            Cell::Pane(p) => p.pane.clone(),
            _ => return Err(MultiplexerError::InvalidCellType(a.clone())),
        };
        let pane_b = match self.cell(b)? {
            Cell::Pane(p) => p.pane.clone(),
            _ => return Err(MultiplexerError::InvalidCellType(b.clone())),
        };
        if let Cell::Pane(p) = self.cell_mut(a)? {
            p.pane = pane_b;
        }
        if let Cell::Pane(p) = self.cell_mut(b)? {
            p.pane = pane_a;
        }
        Ok(())
    }

    fn collect_panes(&self, id: &CellId, out: &mut Vec<PaneId>) -> MultiplexerResult {
        match self.cell(id)? {
            Cell::Root(r) => {
                let child = r.child.clone();
                self.collect_panes(&child, out)?;
            }
            Cell::Split(s) => {
                let lhs = s.lhs_cell.clone();
                let rhs = s.rhs_cell.clone();
                self.collect_panes(&lhs, out)?;
                self.collect_panes(&rhs, out)?;
            }
            Cell::Pane(p) => out.push(p.pane.clone()),
        }
        Ok(())
    }

    /// Normalize `lhs_weight` / `rhs_weight` into a 0..1 ratio. Returns `0.5`
    /// when both weights are zero so callers do not need to special-case it.
    pub fn split_ratio(lhs_weight: f32, rhs_weight: f32) -> f32 {
        let total = lhs_weight + rhs_weight;
        if total == 0.0 {
            0.5
        } else {
            lhs_weight / total
        }
    }

    /// Compute each pane's normalized rectangle.
    /// Returns leaves in DFS left-to-right order, matching `pane_ids_in_subtree`.
    ///
    /// # Invariants
    ///
    /// - Siblings under one `Cell::Split` are produced contiguously and their
    ///   bounds sum to the parent rect because `rhs_size = parent_size - lhs_size`.
    pub(crate) fn pane_bounds(&self, root: &CellId) -> MultiplexerResult<Vec<(PaneId, Rect)>> {
        let mut out = Vec::new();
        self.walk_bounds(
            root,
            Rect {
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 1.0,
            },
            &mut out,
        )?;
        Ok(out)
    }

    /// Recursive worker for `pane_bounds`.
    /// Descends the cell subtree rooted at `id` and pushes one `(PaneId, Rect)` per leaf into `out`,
    /// with `bounds` as the rectangle allotted to this subtree.
    ///
    /// - `Cell::Pane` is a leaf: emit `(pane, bounds)`.
    /// - `Cell::Root` passes `bounds` straight through to its single child.
    /// - `Cell::Split` divides `bounds` along its orientation using
    ///   `Self::split_ratio` and recurses into lhs then rhs. The trailing
    ///   side's size is computed as `parent_size - lhs_size` (not as a
    ///   separate multiplication) so siblings always sum exactly to the
    ///   parent rect — `Window::pane_in_direction`'s adjacency test relies
    ///   on this exactness.
    ///
    /// The lhs-before-rhs descent fixes the output as DFS left-to-right
    /// order, matching `pane_ids_in_subtree`. `pick_best` consumes that
    /// order to break ties on equal `active_point`.
    ///
    /// Each arm clones the child `CellId`s (and `orientation` for splits)
    /// before recursing so the immutable borrow taken by `self.cell(id)?`
    /// is released before the recursive `&self` reborrow.
    fn walk_bounds(
        &self,
        id: &CellId,
        bounds: Rect,
        out: &mut Vec<(PaneId, Rect)>,
    ) -> MultiplexerResult<()> {
        match self.cell(id)? {
            Cell::Pane(p) => {
                out.push((p.pane.clone(), bounds));
                Ok(())
            }
            Cell::Root(r) => {
                let child = r.child.clone();
                self.walk_bounds(&child, bounds, out)
            }
            Cell::Split(s) => {
                let orientation = s.orientation;
                let ratio = Self::split_ratio(s.lhs_weight, s.rhs_weight);
                let lhs_cell = s.lhs_cell.clone();
                let rhs_cell = s.rhs_cell.clone();
                match orientation {
                    SplitOrientation::Horizontal => {
                        let lhs_w = bounds.w * ratio;
                        self.walk_bounds(
                            &lhs_cell,
                            Rect {
                                x: bounds.x,
                                y: bounds.y,
                                w: lhs_w,
                                h: bounds.h,
                            },
                            out,
                        )?;
                        self.walk_bounds(
                            &rhs_cell,
                            Rect {
                                x: bounds.x + lhs_w,
                                y: bounds.y,
                                w: bounds.w - lhs_w,
                                h: bounds.h,
                            },
                            out,
                        )
                    }
                    SplitOrientation::Vertical => {
                        let lhs_h = bounds.h * ratio;
                        self.walk_bounds(
                            &lhs_cell,
                            Rect {
                                x: bounds.x,
                                y: bounds.y,
                                w: bounds.w,
                                h: lhs_h,
                            },
                            out,
                        )?;
                        self.walk_bounds(
                            &rhs_cell,
                            Rect {
                                x: bounds.x,
                                y: bounds.y + lhs_h,
                                w: bounds.w,
                                h: bounds.h - lhs_h,
                            },
                            out,
                        )
                    }
                }
            }
        }
    }

    /// Drop every cell in `start`'s subtree (including `start` itself).
    /// Used during window close to vacate the cell store atomically.
    pub fn remove_subtree(&mut self, start: &CellId) -> MultiplexerResult {
        let mut ids = Vec::new();
        self.collect_cell_ids(start, &mut ids)?;
        for id in ids {
            self.0.remove(&id);
        }
        Ok(())
    }

    fn collect_cell_ids(&self, id: &CellId, out: &mut Vec<CellId>) -> MultiplexerResult {
        out.push(id.clone());
        match self.cell(id)? {
            Cell::Root(r) => {
                let child = r.child.clone();
                self.collect_cell_ids(&child, out)?;
            }
            Cell::Split(s) => {
                let lhs = s.lhs_cell.clone();
                let rhs = s.rhs_cell.clone();
                self.collect_cell_ids(&lhs, out)?;
                self.collect_cell_ids(&rhs, out)?;
            }
            Cell::Pane(_) => {}
        }
        Ok(())
    }

    #[inline]
    pub fn cell(&self, id: &CellId) -> MultiplexerResult<&Cell> {
        self.0
            .get(id)
            .ok_or_else(|| MultiplexerError::CellNotFound(id.clone()))
    }

    #[inline]
    fn cell_mut(&mut self, id: &CellId) -> MultiplexerResult<&mut Cell> {
        self.0
            .get_mut(id)
            .ok_or_else(|| MultiplexerError::CellNotFound(id.clone()))
    }

    /// Sibling-module accessor returning the cell as an `Option`. Used by the
    /// `resize` algorithm which routinely needs to short-circuit on missing
    /// cells without manufacturing a `MultiplexerError`.
    #[inline]
    pub(super) fn get(&self, id: &CellId) -> Option<&Cell> {
        self.0.get(id)
    }

    /// Sibling-module mutable accessor counterpart to [`Self::get`].
    #[inline]
    pub(super) fn get_mut(&mut self, id: &CellId) -> Option<&mut Cell> {
        self.0.get_mut(id)
    }
}

/// Internal record gathered by `plan_collapse` and consumed by `apply_collapse`.
/// Constructed only inside this module, so its existence implies every reference
/// inside has been verified against the store.
#[derive(Debug)]
struct CollapsePlan {
    parent_id: CellId,
    sibling_id: CellId,
    grandparent_id: CellId,
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize, NewType)]
#[newtype(as_ref(str), display, new(uuid_v4_string), default)]
pub struct CellId(String);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Cell {
    Root(RootCell),
    Pane(PaneCell),
    Split(SplitCell),
}

impl Cell {
    pub fn parent(&self) -> Option<&CellId> {
        match self {
            Self::Root(_) => None,
            Self::Pane(c) => c.parent.as_ref(),
            Self::Split(c) => c.parent.as_ref(),
        }
    }

    pub fn require_parent(&self) -> MultiplexerResult<CellId> {
        self.parent()
            .cloned()
            .ok_or(MultiplexerError::MissingParentCell)
    }

    fn set_parent(&mut self, parent: Option<CellId>) {
        match self {
            Self::Pane(c) => c.parent = parent,
            Self::Split(c) => c.parent = parent,
            Self::Root(_) => unreachable!("Cell::Root has no parent"),
        }
    }

    /// Replace a downward child reference. `Root` swaps `child`; `Split` swaps
    /// whichever of `lhs_cell` / `rhs_cell` matches `old`. Calling on `Pane` or
    /// with an `old` not currently occupying a child slot is an invariant
    /// violation.
    fn replace_child(&mut self, old: &CellId, new: CellId) {
        match self {
            Self::Root(r) => {
                debug_assert_eq!(&r.child, old, "root child invariant");
                r.child = new;
            }
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
pub struct RootCell {
    pub child: CellId,
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
/// Callers must handle every variant. `#[must_use]` is a lint-level nudge;
/// type-level enforcement comes from requiring the caller to consume the value
/// via `match`.
#[must_use]
#[derive(Debug, Clone, PartialEq)]
pub enum CloseOutcome {
    /// Target's grandparent was a split; survivor occupies the same lhs/rhs
    /// slot of `new_parent` that the deleted parent occupied (slot pinning).
    SiblingPromoted {
        survivor: CellId,
        new_parent: CellId,
    },
    /// Target's grandparent was the window's `Cell::Root`; survivor was promoted
    /// to be `RootCell::child`. `Window.root_cell` itself is unchanged.
    PromotedToRootChild { survivor: CellId, root: CellId },
}

impl CloseOutcome {
    /// The cell that took the closed target's place in the tree. May be a
    /// `Cell::Pane` or a `Cell::Split` subtree, depending on what was sitting
    /// next to the closed pane.
    pub fn survivor(&self) -> &CellId {
        match self {
            Self::SiblingPromoted { survivor, .. } => survivor,
            Self::PromotedToRootChild { survivor, .. } => survivor,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pid() -> PaneId {
        PaneId::new()
    }

    #[test]
    fn new_window_layout_creates_root_with_child() {
        let mut state = LayoutCellState::default();
        let pane_id = pid();
        let (root_id, pane_cell_id) = state.new_window_layout(pane_id.clone());

        let Cell::Root(root) = state.cell(&root_id).unwrap() else {
            panic!("expected Root");
        };
        assert_eq!(root.child, pane_cell_id);
        let Cell::Pane(pane) = state.cell(&pane_cell_id).unwrap() else {
            panic!("expected Pane");
        };
        assert_eq!(pane.parent.as_ref(), Some(&root_id));
        assert_eq!(pane.pane, pane_id);
    }

    #[test]
    fn split_cell_under_root_updates_root_child() {
        let mut state = LayoutCellState::default();
        let (root_id, pane_a) = state.new_window_layout(pid());
        let pane_b = state.new_pane(pid(), None);

        let split_id = state
            .split_cell(
                pane_a.clone(),
                pane_b.clone(),
                Side::After,
                SplitOrientation::Horizontal,
            )
            .unwrap();

        let Cell::Root(root) = state.cell(&root_id).unwrap() else {
            panic!()
        };
        assert_eq!(root.child, split_id);
        let Cell::Split(split) = state.cell(&split_id).unwrap() else {
            panic!()
        };
        assert_eq!(split.parent.as_ref(), Some(&root_id));
        assert_eq!(split.lhs_cell, pane_a);
        assert_eq!(split.rhs_cell, pane_b);
    }

    #[test]
    fn split_cell_under_split_updates_parent_split_slot() {
        let mut state = LayoutCellState::default();
        let (_, pane_a) = state.new_window_layout(pid());
        let pane_b = state.new_pane(pid(), None);
        let outer = state
            .split_cell(
                pane_a.clone(),
                pane_b.clone(),
                Side::After,
                SplitOrientation::Horizontal,
            )
            .unwrap();

        let pane_c = state.new_pane(pid(), None);
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
    fn close_cell_rejects_last_pane_under_root() {
        let mut state = LayoutCellState::default();
        let (_, pane_cell) = state.new_window_layout(pid());

        let result = state.close_cell(&pane_cell);
        assert!(matches!(
            result,
            Err(MultiplexerError::CannotCloseLastPane(_))
        ));
    }

    #[test]
    fn close_cell_under_root_split_promotes_sibling_to_root_child() {
        let mut state = LayoutCellState::default();
        let (root_id, pane_a) = state.new_window_layout(pid());
        let pane_b = state.new_pane(pid(), None);
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
            CloseOutcome::PromotedToRootChild {
                survivor: pane_b.clone(),
                root: root_id.clone(),
            }
        );

        let Cell::Root(root) = state.cell(&root_id).unwrap() else {
            panic!()
        };
        assert_eq!(root.child, pane_b);
        let Cell::Pane(survivor) = state.cell(&pane_b).unwrap() else {
            panic!()
        };
        assert_eq!(survivor.parent.as_ref(), Some(&root_id));
        assert!(state.cell(&split_id).is_err());
    }

    #[test]
    fn close_cell_under_nested_split_promotes_sibling_in_grandparent_slot() {
        let mut state = LayoutCellState::default();
        let (_, pane_a) = state.new_window_layout(pid());
        let pane_b = state.new_pane(pid(), None);
        let outer = state
            .split_cell(
                pane_a.clone(),
                pane_b.clone(),
                Side::After,
                SplitOrientation::Horizontal,
            )
            .unwrap();
        let pane_c = state.new_pane(pid(), None);
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

    #[test]
    fn pane_ids_in_subtree_collects_all_leaves() {
        let mut state = LayoutCellState::default();
        let (root_id, pane_a) = state.new_window_layout(pid());
        let pane_b = state.new_pane(pid(), None);
        let outer = state
            .split_cell(
                pane_a.clone(),
                pane_b.clone(),
                Side::After,
                SplitOrientation::Horizontal,
            )
            .unwrap();
        let pane_c = state.new_pane(pid(), None);
        state
            .split_cell(
                pane_b.clone(),
                pane_c.clone(),
                Side::After,
                SplitOrientation::Vertical,
            )
            .unwrap();

        let mut ids = state.pane_ids_in_subtree(&root_id).unwrap();
        ids.sort();
        let mut expected: Vec<_> = [&pane_a, &pane_b, &pane_c]
            .iter()
            .map(|c| match state.cell(c).unwrap() {
                Cell::Pane(p) => p.pane.clone(),
                _ => unreachable!(),
            })
            .collect();
        expected.sort();
        assert_eq!(ids, expected);
        let _ = outer;
    }

    #[test]
    fn pane_bounds_single_pane_fills_unit_rect() {
        let mut state = LayoutCellState::default();
        let p = pid();
        let (root_id, _) = state.new_window_layout(p.clone());

        let bounds = state.pane_bounds(&root_id).unwrap();
        assert_eq!(bounds.len(), 1);
        assert_eq!(bounds[0].0, p);
        assert_eq!(
            bounds[0].1,
            Rect {
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 1.0
            }
        );
    }

    #[test]
    fn pane_bounds_horizontal_split_returns_left_then_right_halves() {
        let mut state = LayoutCellState::default();
        let (root_id, lhs) = state.new_window_layout(pid());
        let rhs = state.new_pane(pid(), None);
        state
            .split_cell(
                lhs.clone(),
                rhs.clone(),
                Side::After,
                SplitOrientation::Horizontal,
            )
            .unwrap();

        let bounds = state.pane_bounds(&root_id).unwrap();
        assert_eq!(bounds.len(), 2);
        let lhs_pane = match state.cell(&lhs).unwrap() {
            Cell::Pane(p) => p.pane.clone(),
            _ => unreachable!(),
        };
        let rhs_pane = match state.cell(&rhs).unwrap() {
            Cell::Pane(p) => p.pane.clone(),
            _ => unreachable!(),
        };
        assert_eq!(
            bounds[0],
            (
                lhs_pane,
                Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 0.5,
                    h: 1.0
                }
            )
        );
        assert_eq!(
            bounds[1],
            (
                rhs_pane,
                Rect {
                    x: 0.5,
                    y: 0.0,
                    w: 0.5,
                    h: 1.0
                }
            )
        );
    }

    #[test]
    fn pane_bounds_vertical_split_stacks_top_then_bottom() {
        let mut state = LayoutCellState::default();
        let (root_id, top) = state.new_window_layout(pid());
        let bottom = state.new_pane(pid(), None);
        state
            .split_cell(
                top.clone(),
                bottom.clone(),
                Side::After,
                SplitOrientation::Vertical,
            )
            .unwrap();

        let bounds = state.pane_bounds(&root_id).unwrap();
        let top_pane = match state.cell(&top).unwrap() {
            Cell::Pane(p) => p.pane.clone(),
            _ => unreachable!(),
        };
        let bottom_pane = match state.cell(&bottom).unwrap() {
            Cell::Pane(p) => p.pane.clone(),
            _ => unreachable!(),
        };
        assert_eq!(
            bounds[0],
            (
                top_pane,
                Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 1.0,
                    h: 0.5
                }
            )
        );
        assert_eq!(
            bounds[1],
            (
                bottom_pane,
                Rect {
                    x: 0.0,
                    y: 0.5,
                    w: 1.0,
                    h: 0.5
                }
            )
        );
    }

    #[test]
    fn split_ratio_normalizes_zero_total_to_half() {
        assert_eq!(LayoutCellState::split_ratio(0.0, 0.0), 0.5);
        assert_eq!(LayoutCellState::split_ratio(1.0, 3.0), 0.25);
    }

    #[test]
    fn ordered_pane_cells_returns_cell_id_and_pane_id_in_dfs_order() {
        let mut state = LayoutCellState::default();
        let pa = PaneId::new();
        let pb = PaneId::new();
        let pc = PaneId::new();
        let (root, cell_a) = state.new_window_layout(pa.clone());
        let cell_b = state.new_pane(pb.clone(), None);
        let split_ab = state
            .split_cell(
                cell_a.clone(),
                cell_b.clone(),
                Side::After,
                SplitOrientation::Horizontal,
            )
            .unwrap();
        let cell_c = state.new_pane(pc.clone(), None);
        let _split_abc = state
            .split_cell(
                split_ab,
                cell_c.clone(),
                Side::After,
                SplitOrientation::Vertical,
            )
            .unwrap();

        let ordered = state.ordered_pane_cells(&root).unwrap();
        assert_eq!(ordered, vec![(cell_a, pa), (cell_b, pb), (cell_c, pc)]);
    }

    #[test]
    fn remove_subtree_drops_every_cell_below_root() {
        let mut state = LayoutCellState::default();
        let (root_id, pane_a) = state.new_window_layout(pid());
        let pane_b = state.new_pane(pid(), None);
        let split_id = state
            .split_cell(
                pane_a.clone(),
                pane_b.clone(),
                Side::After,
                SplitOrientation::Horizontal,
            )
            .unwrap();

        state.remove_subtree(&root_id).unwrap();
        assert!(state.cell(&root_id).is_err());
        assert!(state.cell(&split_id).is_err());
        assert!(state.cell(&pane_a).is_err());
        assert!(state.cell(&pane_b).is_err());
    }

    #[test]
    fn swap_panes_exchanges_pane_field_between_two_pane_cells() {
        let mut state = LayoutCellState::default();
        let pa = PaneId::new();
        let pb = PaneId::new();
        let (_root, cell_a) = state.new_window_layout(pa.clone());
        let cell_b = state.new_pane(pb.clone(), None);
        let _split = state
            .split_cell(
                cell_a.clone(),
                cell_b.clone(),
                Side::After,
                SplitOrientation::Horizontal,
            )
            .unwrap();

        state.swap_panes(&cell_a, &cell_b).unwrap();

        match state.cell(&cell_a).unwrap() {
            Cell::Pane(p) => assert_eq!(p.pane, pb),
            _ => panic!("cell_a should still be a Pane"),
        }
        match state.cell(&cell_b).unwrap() {
            Cell::Pane(p) => assert_eq!(p.pane, pa),
            _ => panic!("cell_b should still be a Pane"),
        }
    }

    #[test]
    fn swap_panes_errors_when_either_cell_is_not_a_pane() {
        let mut state = LayoutCellState::default();
        let pa = PaneId::new();
        let pb = PaneId::new();
        let (root, cell_a) = state.new_window_layout(pa.clone());
        let cell_b = state.new_pane(pb.clone(), None);
        let split = state
            .split_cell(
                cell_a.clone(),
                cell_b,
                Side::After,
                SplitOrientation::Horizontal,
            )
            .unwrap();

        let err = state.swap_panes(&cell_a, &split).unwrap_err();
        assert!(matches!(err, MultiplexerError::InvalidCellType(_)));

        let err = state.swap_panes(&root, &cell_a).unwrap_err();
        assert!(matches!(err, MultiplexerError::InvalidCellType(_)));
    }
}
