//! Layout cell tree. Owns the BSP-style split layout for one Session.
//! Pane references inside the tree are Bevy `Entity` values; the
//! internal `pane_to_cell` index maps each Pane entity back to its
//! `CellId` so split / close / swap operations are O(1) lookups.

use crate::error::{MultiplexerError, MultiplexerResult};
use bevy::ecs::entity::Entity;
use std::collections::HashMap;

/// Counter-newtype identifier for a Cell inside `LayoutCellState`. Cells
/// are minted by mutator methods; the counter restarts at 0 for each new
/// `LayoutCellState` instance (only Session-local uniqueness matters).
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Ord, PartialOrd, Default)]
pub struct CellId(u64);

impl std::fmt::Display for CellId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "cell#{}", self.0)
    }
}

/// Layout tree node. `Root` has exactly one child; `Split` has two
/// children at a fixed orientation; `Pane` is a leaf referencing a
/// Bevy Pane entity.
#[derive(Debug, Clone)]
pub enum Cell {
    Root(RootCell),
    Pane(PaneCell),
    Split(SplitCell),
}

impl Cell {
    /// Returns the parent `CellId` of this cell, or `None` for `Cell::Root`.
    pub fn parent(&self) -> Option<&CellId> {
        match self {
            Self::Root(_) => None,
            Self::Pane(c) => c.parent.as_ref(),
            Self::Split(c) => c.parent.as_ref(),
        }
    }

    /// Returns the parent `CellId`, or `MissingParentCell` if absent.
    pub fn require_parent(&self) -> MultiplexerResult<CellId> {
        self.parent()
            .copied()
            .ok_or(MultiplexerError::MissingParentCell)
    }

    fn set_parent(&mut self, parent: Option<CellId>) {
        match self {
            Self::Pane(c) => c.parent = parent,
            Self::Split(c) => c.parent = parent,
            Self::Root(_) => unreachable!("Cell::Root has no parent"),
        }
    }

    /// Replace a downward child reference. `Root` swaps `child`; `Split`
    /// swaps whichever of `lhs_cell` / `rhs_cell` matches `old`. Calling
    /// on `Pane` or with an `old` not currently occupying a child slot is
    /// an invariant violation.
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

/// Layout root. Holds the single child cell. There is exactly one
/// `Root` per `LayoutCellState`.
#[derive(Debug, Clone)]
pub struct RootCell {
    /// The single child of this root node.
    pub child: CellId,
}

/// Leaf cell referencing a Pane entity. Multiple `PaneCell`s in the
/// same `LayoutCellState` may NOT reference the same entity (enforced
/// by `pane_to_cell` invariants).
#[derive(Debug, Clone)]
pub struct PaneCell {
    /// Parent cell id; `None` for orphaned panes before they are attached.
    pub parent: Option<CellId>,
    /// The Bevy Pane entity this leaf represents.
    pub pane: Entity,
}

/// Two-child node oriented horizontally or vertically. `lhs_weight` and
/// `rhs_weight` are the relative weights of each child; `split_ratio()`
/// normalizes them to a `[0,1]` fraction.
#[derive(Debug, Clone)]
pub struct SplitCell {
    /// Parent cell id; `None` for orphaned splits during tree rewiring.
    pub parent: Option<CellId>,
    /// Axis of the split.
    pub orientation: SplitOrientation,
    /// Left or top child.
    pub lhs_cell: CellId,
    /// Relative weight of the lhs child.
    pub lhs_weight: f32,
    /// Right or bottom child.
    pub rhs_cell: CellId,
    /// Relative weight of the rhs child.
    pub rhs_weight: f32,
}

impl SplitCell {
    /// Constructs a new `SplitCell` with equal 0.5/0.5 weights.
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

    /// Returns the sibling of `id` among `lhs_cell` and `rhs_cell`.
    pub fn sibling_cell_id(&self, id: &CellId) -> &CellId {
        if &self.lhs_cell == id {
            &self.rhs_cell
        } else {
            &self.lhs_cell
        }
    }
}

/// Split axis.
#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq)]
pub enum SplitOrientation {
    /// Left and right children share horizontal space.
    Horizontal,
    /// Top and bottom children share vertical space.
    Vertical,
}

/// Which side of an existing cell a newly-inserted sibling lands on.
#[derive(Debug, Default, Clone, Copy, Hash, Eq, PartialEq)]
pub enum Side {
    /// Place new_cell before target (left or top, depending on orientation).
    Before,
    /// Place new_cell after target (right or bottom, depending on orientation).
    #[default]
    After,
}

/// Structural outcome of `LayoutCellState::close_cell`.
///
/// Callers must handle every variant. `#[must_use]` nudges the caller to
/// inspect the result; type-level enforcement comes from requiring a `match`.
#[must_use]
#[derive(Debug, Clone, PartialEq)]
pub enum CloseOutcome {
    /// Target's grandparent was a split; survivor occupies the same lhs/rhs
    /// slot of `new_parent` that the deleted parent occupied (slot pinning).
    SiblingPromoted {
        /// The cell that took the closed target's place in the tree.
        survivor: CellId,
        /// The grandparent split that now directly owns the survivor.
        new_parent: CellId,
    },
    /// Target's grandparent was the session's `Cell::Root`; survivor was
    /// promoted to be `RootCell::child`. `Session.root_cell` itself is unchanged.
    PromotedToRootChild {
        /// The cell that took the closed target's place in the tree.
        survivor: CellId,
        /// The root cell whose `child` now points to the survivor.
        root: CellId,
    },
}

impl CloseOutcome {
    /// The cell that took the closed target's place in the tree. May be a
    /// `Cell::Pane` or a `Cell::Split` subtree.
    pub fn survivor(&self) -> &CellId {
        match self {
            Self::SiblingPromoted { survivor, .. } | Self::PromotedToRootChild { survivor, .. } => {
                survivor
            }
        }
    }
}

/// Axis-aligned rectangle in normalized session coordinates (`x, y, w, h` ∈ [0, 1]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    /// Left edge in [0, 1].
    pub x: f32,
    /// Top edge in [0, 1].
    pub y: f32,
    /// Width in [0, 1].
    pub w: f32,
    /// Height in [0, 1].
    pub h: f32,
}

/// Internal record gathered by `plan_collapse` and consumed by `apply_collapse`.
/// Constructed only inside this module, so its existence implies every
/// referenced cell has been verified against the store.
#[derive(Debug)]
struct CollapsePlan {
    parent_id: CellId,
    sibling_id: CellId,
    grandparent_id: CellId,
}

/// Layout cell store plus pane-to-cell index. Owned as a component on
/// each Session entity. Mutator methods keep the index in sync
/// transactionally.
#[derive(Debug, Default, Clone)]
pub struct LayoutCellState {
    cells: HashMap<CellId, Cell>,
    pane_to_cell: HashMap<Entity, CellId>,
    next_cell_id: u64,
}

impl LayoutCellState {
    /// Initialize a new Session's layout: a `Cell::Root` and its single
    /// initial `Cell::Pane`, registered atomically so `RootCell::child`
    /// is always valid. Returns `(root_cell_id, initial_pane_cell_id)`.
    pub fn new_session_layout(&mut self, pane: Entity) -> (CellId, CellId) {
        let root_id = self.mint_cell_id();
        let pane_cell_id = self.mint_cell_id();
        self.cells.insert(
            pane_cell_id,
            Cell::Pane(PaneCell {
                parent: Some(root_id),
                pane,
            }),
        );
        self.cells.insert(
            root_id,
            Cell::Root(RootCell {
                child: pane_cell_id,
            }),
        );
        self.pane_to_cell.insert(pane, pane_cell_id);
        (root_id, pane_cell_id)
    }

    /// Register a pane cell. `parent == None` produces an orphan that
    /// callers (typically `split_cell`) are expected to attach into the
    /// tree shortly after creation.
    pub fn new_pane(&mut self, pane: Entity, parent: Option<CellId>) -> CellId {
        let id = self.mint_cell_id();
        self.cells.insert(id, Cell::Pane(PaneCell { parent, pane }));
        self.pane_to_cell.insert(pane, id);
        id
    }

    /// Insert `new_cell` as a sibling of `target` under a fresh `Split` node.
    /// `target` may be any cell (Pane or Split) — the new Split node takes
    /// `target`'s former parent position; `target` and `new_cell` become the
    /// new Split's children, ordered by `new_cell_side`. The new cell must
    /// already exist in the layout (typically just minted via `new_pane`).
    /// Returns the newly-created `Cell::Split` node.
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

        let split_id = self.mint_cell_id();
        self.cell_mut(&target)?.set_parent(Some(split_id));
        self.cell_mut(&new_cell)?.set_parent(Some(split_id));
        self.cell_mut(&target_parent)?
            .replace_child(&target, split_id);

        let (lhs_cell, rhs_cell) = match new_cell_side {
            Side::Before => (new_cell, target),
            Side::After => (target, new_cell),
        };
        self.cells.insert(
            split_id,
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

    /// Look up a cell by id.
    pub fn cell(&self, id: &CellId) -> MultiplexerResult<&Cell> {
        self.cells
            .get(id)
            .ok_or(MultiplexerError::CellNotFound(*id))
    }

    /// Return the `CellId` for a given Pane entity.
    pub fn lookup_cell_for_pane(&self, pane: Entity) -> MultiplexerResult<CellId> {
        self.pane_to_cell
            .get(&pane)
            .copied()
            .ok_or(MultiplexerError::CellForPaneNotFound(pane))
    }

    /// Walk down `start`'s subtree along the lhs/child branch and return the
    /// first `Cell::Pane` reached. Used to pick a representative pane entity when
    /// promoting a survivor subtree to be active.
    pub fn leftmost_pane(&self, start: &CellId) -> MultiplexerResult<Entity> {
        let mut current = *start;
        loop {
            match self.cell(&current)? {
                Cell::Pane(c) => return Ok(c.pane),
                Cell::Split(s) => current = s.lhs_cell,
                Cell::Root(r) => current = r.child,
            }
        }
    }

    /// Collect every `Entity` reachable from `start`'s subtree, in
    /// depth-first lhs-before-rhs order.
    pub fn pane_entities_in_subtree(&self, start: &CellId) -> MultiplexerResult<Vec<Entity>> {
        Ok(self
            .ordered_pane_cells(start)?
            .into_iter()
            .map(|(_, p)| p)
            .collect())
    }

    /// Collect every pane leaf reachable from `start`, yielding each leaf's
    /// (`CellId`, `Entity`) in depth-first lhs-before-rhs order.
    pub fn ordered_pane_cells(&self, start: &CellId) -> MultiplexerResult<Vec<(CellId, Entity)>> {
        let mut out = Vec::new();
        self.collect_pane_cells(start, &mut out)?;
        Ok(out)
    }

    /// Swap the `pane:` field of two `PaneCell` leaves. Errors with
    /// `InvalidCellType` if either id resolves to `Cell::Root` or `Cell::Split`.
    /// Cell ids, parent pointers, splits, and weights are untouched — only the
    /// pane payload of each cell moves.
    pub fn swap_panes(&mut self, a: &CellId, b: &CellId) -> MultiplexerResult<()> {
        if a == b {
            return Ok(());
        }
        let [Some(cell_a), Some(cell_b)] = self.cells.get_disjoint_mut([a, b]) else {
            return Err(MultiplexerError::CellNotFound(*a));
        };
        let (Cell::Pane(pa), Cell::Pane(pb)) = (cell_a, cell_b) else {
            return Err(MultiplexerError::InvalidCellType(*a));
        };
        let old_pa = pa.pane;
        let old_pb = pb.pane;
        std::mem::swap(&mut pa.pane, &mut pb.pane);
        self.pane_to_cell.insert(old_pa, *b);
        self.pane_to_cell.insert(old_pb, *a);
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
    /// Returns leaves in DFS left-to-right order, matching `pane_entities_in_subtree`.
    ///
    /// # Invariants
    ///
    /// - Siblings under one `Cell::Split` are produced contiguously and their
    ///   bounds sum to the parent rect because `rhs_size = parent_size - lhs_size`.
    pub fn pane_bounds(&self, root: &CellId) -> MultiplexerResult<Vec<(Entity, Rect)>> {
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

    /// Drop every cell in `start`'s subtree (including `start` itself).
    /// Used during session close to vacate the cell store atomically.
    pub fn remove_subtree(&mut self, start: &CellId) -> MultiplexerResult<()> {
        let panes = self.pane_entities_in_subtree(start)?;
        for pane in panes {
            self.pane_to_cell.remove(&pane);
        }
        let mut ids = Vec::new();
        self.collect_cell_ids(start, &mut ids)?;
        for id in ids {
            self.cells.remove(&id);
        }
        Ok(())
    }

    fn mint_cell_id(&mut self) -> CellId {
        let id = CellId(self.next_cell_id);
        self.next_cell_id = self
            .next_cell_id
            .checked_add(1)
            .expect("CellId u64 overflow");
        id
    }

    pub(crate) fn cell_mut(&mut self, id: &CellId) -> MultiplexerResult<&mut Cell> {
        self.cells
            .get_mut(id)
            .ok_or(MultiplexerError::CellNotFound(*id))
    }

    fn target_pane_parent(&self, id: &CellId) -> MultiplexerResult<CellId> {
        match self.cell(id)? {
            Cell::Pane(p) => p.parent.ok_or(MultiplexerError::MissingParentCell),
            _ => Err(MultiplexerError::InvalidCellType(*id)),
        }
    }

    fn plan_collapse(
        &self,
        target_id: &CellId,
        parent_id: CellId,
    ) -> MultiplexerResult<CollapsePlan> {
        let parent_split = match self.cell(&parent_id)? {
            Cell::Split(s) => s,
            // NOTE: Pane is the only child of Root — closing it would empty the
            // session's layout, which the model forbids.
            Cell::Root(_) => {
                return Err(MultiplexerError::CannotCloseLastPane(*target_id));
            }
            Cell::Pane(_) => return Err(MultiplexerError::InvalidCellType(parent_id)),
        };
        let sibling_id = *parent_split.sibling_cell_id(target_id);
        let grandparent_id = parent_split
            .parent
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
        if let Some(Cell::Pane(p)) = self.cells.get(target_id) {
            self.pane_to_cell.remove(&p.pane);
        }
        self.cells.remove(target_id);
        self.cells
            .remove(&plan.parent_id)
            .expect("parent existed in plan");
        self.cell_mut(&plan.sibling_id)
            .expect("sibling existed in plan")
            .set_parent(Some(plan.grandparent_id));

        let grandparent = self
            .cells
            .get_mut(&plan.grandparent_id)
            .expect("grandparent existed in plan");
        let promoted_to_root = matches!(grandparent, Cell::Root(_));
        grandparent.replace_child(&plan.parent_id, plan.sibling_id);

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

    fn collect_pane_cells(
        &self,
        id: &CellId,
        out: &mut Vec<(CellId, Entity)>,
    ) -> MultiplexerResult<()> {
        match self.cell(id)? {
            Cell::Root(r) => {
                let child = r.child;
                self.collect_pane_cells(&child, out)?;
            }
            Cell::Split(s) => {
                let lhs = s.lhs_cell;
                let rhs = s.rhs_cell;
                self.collect_pane_cells(&lhs, out)?;
                self.collect_pane_cells(&rhs, out)?;
            }
            Cell::Pane(p) => out.push((*id, p.pane)),
        }
        Ok(())
    }

    fn collect_cell_ids(&self, id: &CellId, out: &mut Vec<CellId>) -> MultiplexerResult<()> {
        out.push(*id);
        match self.cell(id)? {
            Cell::Root(r) => {
                let child = r.child;
                self.collect_cell_ids(&child, out)?;
            }
            Cell::Split(s) => {
                let lhs = s.lhs_cell;
                let rhs = s.rhs_cell;
                self.collect_cell_ids(&lhs, out)?;
                self.collect_cell_ids(&rhs, out)?;
            }
            Cell::Pane(_) => {}
        }
        Ok(())
    }

    fn walk_bounds(
        &self,
        id: &CellId,
        bounds: Rect,
        out: &mut Vec<(Entity, Rect)>,
    ) -> MultiplexerResult<()> {
        match self.cell(id)? {
            Cell::Pane(p) => {
                out.push((p.pane, bounds));
                Ok(())
            }
            Cell::Root(r) => {
                let child = r.child;
                self.walk_bounds(&child, bounds, out)
            }
            Cell::Split(s) => {
                let orientation = s.orientation;
                let ratio = Self::split_ratio(s.lhs_weight, s.rhs_weight);
                let lhs_cell = s.lhs_cell;
                let rhs_cell = s.rhs_cell;
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::entity::Entity;

    fn pane(n: u32) -> Entity {
        Entity::from_raw_u32(n).expect("nonzero entity id")
    }

    #[test]
    fn new_session_layout_creates_root_with_child() {
        let mut state = LayoutCellState::default();
        let pane_id = pane(1);
        let (root_id, pane_cell_id) = state.new_session_layout(pane_id);

        let Cell::Root(root) = state.cell(&root_id).unwrap() else {
            panic!("expected Root");
        };
        assert_eq!(root.child, pane_cell_id);
        let Cell::Pane(pane_cell) = state.cell(&pane_cell_id).unwrap() else {
            panic!("expected Pane");
        };
        assert_eq!(pane_cell.parent.as_ref(), Some(&root_id));
        assert_eq!(pane_cell.pane, pane_id);
    }

    #[test]
    fn split_cell_under_root_updates_root_child() {
        let mut state = LayoutCellState::default();
        let (root_id, pane_a) = state.new_session_layout(pane(1));
        let pane_b = state.new_pane(pane(2), None);

        let split_id = state
            .split_cell(pane_a, pane_b, Side::After, SplitOrientation::Horizontal)
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
        let (_, pane_a) = state.new_session_layout(pane(1));
        let pane_b = state.new_pane(pane(2), None);
        let outer = state
            .split_cell(pane_a, pane_b, Side::After, SplitOrientation::Horizontal)
            .unwrap();

        let pane_c = state.new_pane(pane(3), None);
        let inner = state
            .split_cell(pane_b, pane_c, Side::After, SplitOrientation::Vertical)
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
        let (_, pane_cell) = state.new_session_layout(pane(1));

        let result = state.close_cell(&pane_cell);
        assert!(matches!(
            result,
            Err(MultiplexerError::CannotCloseLastPane(_))
        ));
    }

    #[test]
    fn close_cell_under_root_split_promotes_sibling_to_root_child() {
        let mut state = LayoutCellState::default();
        let (root_id, pane_a) = state.new_session_layout(pane(1));
        let pane_b = state.new_pane(pane(2), None);
        let split_id = state
            .split_cell(pane_a, pane_b, Side::After, SplitOrientation::Horizontal)
            .unwrap();

        let outcome = state.close_cell(&pane_a).unwrap();
        assert_eq!(
            outcome,
            CloseOutcome::PromotedToRootChild {
                survivor: pane_b,
                root: root_id,
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
        let (_, pane_a) = state.new_session_layout(pane(1));
        let pane_b = state.new_pane(pane(2), None);
        let outer = state
            .split_cell(pane_a, pane_b, Side::After, SplitOrientation::Horizontal)
            .unwrap();
        let pane_c = state.new_pane(pane(3), None);
        let inner = state
            .split_cell(pane_b, pane_c, Side::After, SplitOrientation::Vertical)
            .unwrap();

        let outcome = state.close_cell(&pane_b).unwrap();
        assert_eq!(
            outcome,
            CloseOutcome::SiblingPromoted {
                survivor: pane_c,
                new_parent: outer,
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
    fn pane_entities_in_subtree_collects_all_leaves() {
        let mut state = LayoutCellState::default();
        let pa = pane(1);
        let pb = pane(2);
        let pc = pane(3);
        let (root_id, pane_a) = state.new_session_layout(pa);
        let pane_b = state.new_pane(pb, None);
        let outer = state
            .split_cell(pane_a, pane_b, Side::After, SplitOrientation::Horizontal)
            .unwrap();
        let pane_c = state.new_pane(pc, None);
        state
            .split_cell(pane_b, pane_c, Side::After, SplitOrientation::Vertical)
            .unwrap();

        let mut entities = state.pane_entities_in_subtree(&root_id).unwrap();
        entities.sort();
        let mut expected = vec![pa, pb, pc];
        expected.sort();
        assert_eq!(entities, expected);
        let _ = outer;
    }

    #[test]
    fn pane_bounds_single_pane_fills_unit_rect() {
        let mut state = LayoutCellState::default();
        let p = pane(1);
        let (root_id, _) = state.new_session_layout(p);

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
        let lhs_pane = pane(1);
        let rhs_pane = pane(2);
        let (root_id, lhs) = state.new_session_layout(lhs_pane);
        let rhs = state.new_pane(rhs_pane, None);
        state
            .split_cell(lhs, rhs, Side::After, SplitOrientation::Horizontal)
            .unwrap();

        let bounds = state.pane_bounds(&root_id).unwrap();
        assert_eq!(bounds.len(), 2);
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
        let top_pane = pane(1);
        let bottom_pane = pane(2);
        let (root_id, top) = state.new_session_layout(top_pane);
        let bottom = state.new_pane(bottom_pane, None);
        state
            .split_cell(top, bottom, Side::After, SplitOrientation::Vertical)
            .unwrap();

        let bounds = state.pane_bounds(&root_id).unwrap();
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
    fn ordered_pane_cells_returns_cell_id_and_entity_in_dfs_order() {
        let mut state = LayoutCellState::default();
        let pa = pane(1);
        let pb = pane(2);
        let pc = pane(3);
        let (root, cell_a) = state.new_session_layout(pa);
        let cell_b = state.new_pane(pb, None);
        let split_ab = state
            .split_cell(cell_a, cell_b, Side::After, SplitOrientation::Horizontal)
            .unwrap();
        let cell_c = state.new_pane(pc, None);
        let _split_abc = state
            .split_cell(split_ab, cell_c, Side::After, SplitOrientation::Vertical)
            .unwrap();

        let ordered = state.ordered_pane_cells(&root).unwrap();
        assert_eq!(ordered, vec![(cell_a, pa), (cell_b, pb), (cell_c, pc)]);
    }

    #[test]
    fn remove_subtree_drops_every_cell_below_root() {
        let mut state = LayoutCellState::default();
        let (root_id, pane_a) = state.new_session_layout(pane(1));
        let pane_b_entity = pane(2);
        let pane_b = state.new_pane(pane_b_entity, None);
        let split_id = state
            .split_cell(pane_a, pane_b, Side::After, SplitOrientation::Horizontal)
            .unwrap();

        state.remove_subtree(&root_id).unwrap();
        assert!(state.cell(&root_id).is_err());
        assert!(state.cell(&split_id).is_err());
        assert!(state.cell(&pane_a).is_err());
        assert!(state.cell(&pane_b).is_err());
        // Verify the pane_to_cell index is also cleaned up.
        assert!(
            state.lookup_cell_for_pane(pane(1)).is_err(),
            "pane_to_cell entry for pane(1) must be removed",
        );
        assert!(
            state.lookup_cell_for_pane(pane_b_entity).is_err(),
            "pane_to_cell entry for pane(2) must be removed",
        );
    }

    #[test]
    fn swap_panes_exchanges_pane_field_between_two_pane_cells() {
        let mut state = LayoutCellState::default();
        let pa = pane(1);
        let pb = pane(2);
        let (_root, cell_a) = state.new_session_layout(pa);
        let cell_b = state.new_pane(pb, None);
        let _split = state
            .split_cell(cell_a, cell_b, Side::After, SplitOrientation::Horizontal)
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
        let pa = pane(1);
        let pb = pane(2);
        let (root, cell_a) = state.new_session_layout(pa);
        let cell_b = state.new_pane(pb, None);
        let split = state
            .split_cell(cell_a, cell_b, Side::After, SplitOrientation::Horizontal)
            .unwrap();

        let err = state.swap_panes(&cell_a, &split).unwrap_err();
        assert!(matches!(err, MultiplexerError::InvalidCellType(_)));

        let err = state.swap_panes(&root, &cell_a).unwrap_err();
        assert!(matches!(err, MultiplexerError::InvalidCellType(_)));
    }

    #[test]
    fn lookup_cell_for_pane_returns_correct_cell() {
        let mut state = LayoutCellState::default();
        let pa = pane(1);
        let pb = pane(2);
        let (_, cell_a) = state.new_session_layout(pa);
        let cell_b = state.new_pane(pb, None);
        state
            .split_cell(cell_a, cell_b, Side::After, SplitOrientation::Horizontal)
            .unwrap();

        assert_eq!(state.lookup_cell_for_pane(pa).unwrap(), cell_a);
        assert_eq!(state.lookup_cell_for_pane(pb).unwrap(), cell_b);
    }

    #[test]
    fn swap_panes_updates_pane_to_cell_index() {
        let mut state = LayoutCellState::default();
        let pa = pane(1);
        let pb = pane(2);
        let (_, cell_a) = state.new_session_layout(pa);
        let cell_b = state.new_pane(pb, None);
        state
            .split_cell(cell_a, cell_b, Side::After, SplitOrientation::Horizontal)
            .unwrap();

        state.swap_panes(&cell_a, &cell_b).unwrap();

        assert_eq!(state.lookup_cell_for_pane(pa).unwrap(), cell_b);
        assert_eq!(state.lookup_cell_for_pane(pb).unwrap(), cell_a);
    }

    #[test]
    fn swap_panes_with_same_cell_id_is_noop() {
        let mut state = LayoutCellState::default();
        let pa = pane(1);
        let (_, cell_a) = state.new_session_layout(pa);

        assert!(state.swap_panes(&cell_a, &cell_a).is_ok());

        // The pane is still where it was.
        assert_eq!(state.lookup_cell_for_pane(pa).unwrap(), cell_a);
        match state.cell(&cell_a).unwrap() {
            Cell::Pane(p) => assert_eq!(p.pane, pa),
            _ => panic!("cell_a should still be a Pane"),
        }
    }

    #[test]
    fn close_cell_removes_pane_from_pane_to_cell_index() {
        let mut state = LayoutCellState::default();
        let pa = pane(1);
        let pb = pane(2);
        let (_, pane_a) = state.new_session_layout(pa);
        let pane_b = state.new_pane(pb, None);
        state
            .split_cell(pane_a, pane_b, Side::After, SplitOrientation::Horizontal)
            .unwrap();

        let _ = state.close_cell(&pane_a).unwrap();

        assert!(state.lookup_cell_for_pane(pa).is_err());
        assert!(state.lookup_cell_for_pane(pb).is_ok());
    }

    #[test]
    fn leftmost_pane_returns_leftmost_leaf() {
        let mut state = LayoutCellState::default();
        let pa = pane(1);
        let pb = pane(2);
        let pc = pane(3);
        let (root, cell_a) = state.new_session_layout(pa);
        let cell_b = state.new_pane(pb, None);
        let split_ab = state
            .split_cell(cell_a, cell_b, Side::After, SplitOrientation::Horizontal)
            .unwrap();
        let cell_c = state.new_pane(pc, None);
        state
            .split_cell(split_ab, cell_c, Side::After, SplitOrientation::Vertical)
            .unwrap();

        assert_eq!(state.leftmost_pane(&root).unwrap(), pa);
    }
}
