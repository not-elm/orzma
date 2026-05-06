use crate::{
    error::{SessionError, SessionResult},
    pane::PaneId,
};
use ozmux_macros::define_string_new_type;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Default, Debug, Serialize)]
pub struct LayoutCellState(HashMap<CellId, LayoutCell>);

impl LayoutCellState {
    #[inline]
    fn insert(&mut self, id: CellId, node: LayoutCell) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pane::PaneId;
    use std::collections::{HashMap, HashSet};

    /// Create a parent-less pane cell, insert it into the store, and return its CellId.
    /// Equivalent to a freshly-created tmux pane (the kind passed as new_cell to split_cell).
    fn new_root_pane(store: &mut LayoutCellState) -> CellId {
        let pane_id = PaneId::new();
        store.create_pane_cell(pane_id, None)
    }

    /// Snapshot the store's internal HashMap by full clone.
    /// Used for "store unchanged" assertions in failure-path tests.
    fn snapshot(store: &LayoutCellState) -> HashMap<CellId, LayoutCell> {
        store.0.clone()
    }

    /// Assert the cell is a Split with the expected (lhs_cell, rhs_cell, orientation),
    /// and that lhs_weight == rhs_weight == 0.5 (the default produced by SplitCell::new).
    fn assert_split(
        store: &LayoutCellState,
        id: &CellId,
        lhs: &CellId,
        rhs: &CellId,
        orientation: SplitOrientation,
    ) {
        let cell = store.cell(id).expect("split cell not found in store");
        let Cell::Split(s) = &cell.cell else {
            panic!("expected Split cell at {id}, got Pane");
        };
        assert_eq!(&s.lhs_cell, lhs, "lhs_cell mismatch at split {id}");
        assert_eq!(&s.rhs_cell, rhs, "rhs_cell mismatch at split {id}");
        assert_eq!(
            s.orientation, orientation,
            "orientation mismatch at split {id}"
        );
        assert_eq!(s.lhs_weight, 0.5, "lhs_weight should be 0.5 at split {id}");
        assert_eq!(s.rhs_weight, 0.5, "rhs_weight should be 0.5 at split {id}");
    }

    /// Assert tree integrity across the whole store.
    ///
    /// Axioms checked:
    /// 1. Every cell whose parent is `Some(p)`: `p` exists in the store, is a `Cell::Split`,
    ///    and lists this cell as either `lhs_cell` or `rhs_cell`.
    /// 2. Each `Split`'s `lhs_cell` / `rhs_cell` exist in the store, and their parent
    ///    points back to the split.
    /// 3. Every non-root cell is referenced by exactly one `Split`; root is referenced 0 times.
    /// 4. (Single-root) If `root` is `Some`, exactly one cell has `parent.is_none()` and it equals `root`.
    ///    If `root` is `None`, the store is empty.
    /// 5. (Reachability) If `root` is `Some`, every cell is reachable from `root` via `lhs`/`rhs`
    ///    descent into `Cell::Split`.
    /// 6. (Acyclicity) The descent visits each cell at most once and terminates within
    ///    `store.len()` steps.
    fn assert_well_formed(store: &LayoutCellState, root: Option<&CellId>) {
        // Axiom 1 & 2: bidirectional parent/child links.
        for (id, cell) in &store.0 {
            if let Some(parent_id) = &cell.parent {
                let parent = store.0.get(parent_id).unwrap_or_else(|| {
                    panic!("cell {id} claims parent {parent_id} but parent missing from store")
                });
                let Cell::Split(p) = &parent.cell else {
                    panic!("cell {id} claims parent {parent_id} but parent is not a Split");
                };
                assert!(
                    &p.lhs_cell == id || &p.rhs_cell == id,
                    "cell {id} claims parent {parent_id} but parent does not list it as a child",
                );
            }

            if let Cell::Split(s) = &cell.cell {
                for child_id in [&s.lhs_cell, &s.rhs_cell] {
                    let child = store.0.get(child_id).unwrap_or_else(|| {
                        panic!("split {id} references missing child {child_id}")
                    });
                    assert_eq!(
                        child.parent.as_ref(),
                        Some(id),
                        "split {id} has child {child_id} but child's parent is {:?}",
                        child.parent,
                    );
                }
            }
        }

        // Axiom 3: reference cardinality.
        let mut ref_count: HashMap<&CellId, usize> = HashMap::new();
        for cell in store.0.values() {
            if let Cell::Split(s) = &cell.cell {
                *ref_count.entry(&s.lhs_cell).or_insert(0) += 1;
                *ref_count.entry(&s.rhs_cell).or_insert(0) += 1;
            }
        }
        for (id, cell) in &store.0 {
            let count = ref_count.get(id).copied().unwrap_or(0);
            if cell.parent.is_some() {
                assert_eq!(
                    count, 1,
                    "non-root cell {id} should be referenced exactly once, found {count}"
                );
            } else {
                assert_eq!(
                    count, 0,
                    "root cell {id} should not be referenced as a child, found {count}"
                );
            }
        }

        // Axiom 4: single-root uniqueness.
        let parentless: Vec<&CellId> = store
            .0
            .iter()
            .filter(|(_, c)| c.parent.is_none())
            .map(|(id, _)| id)
            .collect();
        match root {
            Some(root_id) => {
                assert_eq!(
                    parentless.len(),
                    1,
                    "expected exactly 1 parentless cell when root is Some, found {}: {:?}",
                    parentless.len(),
                    parentless,
                );
                assert_eq!(
                    parentless[0], root_id,
                    "parentless cell {} does not match expected root {}",
                    parentless[0], root_id,
                );
            }
            None => {
                assert!(
                    store.0.is_empty(),
                    "expected empty store when root is None, found {} cells",
                    store.0.len(),
                );
            }
        }

        // Axioms 5 & 6: reachability + acyclicity (DFS from root).
        if let Some(root_id) = root {
            let mut visited: HashSet<CellId> = HashSet::new();
            let mut stack: Vec<CellId> = vec![root_id.clone()];
            while let Some(id) = stack.pop() {
                assert!(
                    visited.insert(id.clone()),
                    "cycle or duplicate reference detected at cell {id}",
                );
                assert!(
                    visited.len() <= store.0.len(),
                    "DFS exceeded store size — possible cycle"
                );
                let cell = store
                    .0
                    .get(&id)
                    .unwrap_or_else(|| panic!("reachability descent reached missing cell {id}"));
                if let Cell::Split(s) = &cell.cell {
                    stack.push(s.lhs_cell.clone());
                    stack.push(s.rhs_cell.clone());
                }
            }
            assert_eq!(
                visited.len(),
                store.0.len(),
                "{} cells reachable from root but store has {} cells",
                visited.len(),
                store.0.len(),
            );
        }
    }

    #[test]
    fn helpers_compile_smoke() {
        let store = LayoutCellState::default();
        assert_well_formed(&store, None);
        let _snap = snapshot(&store);
    }

    #[test]
    fn split_root_pane() {
        let mut store = LayoutCellState::default();
        let lhs = new_root_pane(&mut store);
        let rhs = new_root_pane(&mut store);

        assert_eq!(store.cell(&lhs).unwrap().parent, None);
        assert_eq!(store.cell(&rhs).unwrap().parent, None);

        let split_id = store
            .split_cell(
                lhs.clone(),
                rhs.clone(),
                Side::After,
                SplitOrientation::Horizontal,
            )
            .expect("split should succeed");

        assert_eq!(
            store.cell(&split_id).unwrap().parent,
            None,
            "new split should be root"
        );

        assert_eq!(
            store.cell(&rhs).unwrap().parent.as_ref(),
            Some(&split_id),
            "rhs.parent should now point to new split"
        );
        assert_eq!(
            store.cell(&lhs).unwrap().parent.as_ref(),
            Some(&split_id),
            "lhs.parent should now point to new split"
        );

        assert_split(&store, &split_id, &lhs, &rhs, SplitOrientation::Horizontal);
        assert_well_formed(&store, Some(&split_id));
    }

    #[test]
    fn split_target_in_lhs_position_of_existing_split() {
        let mut store = LayoutCellState::default();
        let a = new_root_pane(&mut store);
        let b = new_root_pane(&mut store);
        let p = store
            .split_cell(
                a.clone(),
                b.clone(),
                Side::After,
                SplitOrientation::Horizontal,
            )
            .expect("first split");

        let new_pane = new_root_pane(&mut store);
        let new_split = store
            .split_cell(
                a.clone(),
                new_pane.clone(),
                Side::After,
                SplitOrientation::Vertical,
            )
            .expect("second split");

        assert_split(&store, &p, &new_split, &b, SplitOrientation::Horizontal);

        assert_split(
            &store,
            &new_split,
            &a,
            &new_pane,
            SplitOrientation::Vertical,
        );

        assert_eq!(store.cell(&new_split).unwrap().parent.as_ref(), Some(&p));
        assert_eq!(
            store.cell(&p).unwrap().parent,
            None,
            "P should still be root"
        );
        assert_eq!(store.cell(&a).unwrap().parent.as_ref(), Some(&new_split));
        assert_eq!(
            store.cell(&new_pane).unwrap().parent.as_ref(),
            Some(&new_split)
        );

        assert_eq!(
            store.cell(&b).unwrap().parent.as_ref(),
            Some(&p),
            "B should be unchanged"
        );

        assert_well_formed(&store, Some(&p));
    }

    #[test]
    fn split_target_in_rhs_position_of_existing_split() {
        let mut store = LayoutCellState::default();
        let a = new_root_pane(&mut store);
        let b = new_root_pane(&mut store);
        let p = store
            .split_cell(
                a.clone(),
                b.clone(),
                Side::After,
                SplitOrientation::Horizontal,
            )
            .expect("first split");

        let new_pane = new_root_pane(&mut store);
        let new_split = store
            .split_cell(
                b.clone(),
                new_pane.clone(),
                Side::After,
                SplitOrientation::Vertical,
            )
            .expect("second split");

        assert_split(&store, &p, &a, &new_split, SplitOrientation::Horizontal);

        assert_split(
            &store,
            &new_split,
            &b,
            &new_pane,
            SplitOrientation::Vertical,
        );

        assert_eq!(store.cell(&new_split).unwrap().parent.as_ref(), Some(&p));
        assert_eq!(store.cell(&p).unwrap().parent, None);
        assert_eq!(store.cell(&b).unwrap().parent.as_ref(), Some(&new_split));
        assert_eq!(
            store.cell(&new_pane).unwrap().parent.as_ref(),
            Some(&new_split)
        );

        assert_eq!(
            store.cell(&a).unwrap().parent.as_ref(),
            Some(&p),
            "A should be unchanged"
        );

        assert_well_formed(&store, Some(&p));
    }

    #[test]
    fn split_twice_deeply() {
        let mut store = LayoutCellState::default();
        let a = new_root_pane(&mut store);
        let b = new_root_pane(&mut store);
        let p_root = store
            .split_cell(
                a.clone(),
                b.clone(),
                Side::After,
                SplitOrientation::Horizontal,
            )
            .unwrap();

        let c = new_root_pane(&mut store);
        let mid = store
            .split_cell(
                a.clone(),
                c.clone(),
                Side::After,
                SplitOrientation::Vertical,
            )
            .unwrap();

        let d = new_root_pane(&mut store);
        let inner = store
            .split_cell(
                a.clone(),
                d.clone(),
                Side::After,
                SplitOrientation::Horizontal,
            )
            .unwrap();

        assert_split(&store, &p_root, &mid, &b, SplitOrientation::Horizontal);
        assert_split(&store, &mid, &inner, &c, SplitOrientation::Vertical);
        assert_split(&store, &inner, &a, &d, SplitOrientation::Horizontal);

        assert_eq!(store.cell(&p_root).unwrap().parent, None);
        assert_eq!(store.cell(&mid).unwrap().parent.as_ref(), Some(&p_root));
        assert_eq!(store.cell(&inner).unwrap().parent.as_ref(), Some(&mid));
        assert_eq!(store.cell(&a).unwrap().parent.as_ref(), Some(&inner));

        assert_eq!(store.cell(&b).unwrap().parent.as_ref(), Some(&p_root));
        assert_eq!(store.cell(&c).unwrap().parent.as_ref(), Some(&mid));
        assert_eq!(store.cell(&d).unwrap().parent.as_ref(), Some(&inner));

        assert_well_formed(&store, Some(&p_root));
    }

    #[test]
    fn split_with_same_lhs_rhs_returns_error() {
        let mut store = LayoutCellState::default();
        let a = new_root_pane(&mut store);

        let result = store.split_cell(
            a.clone(),
            a.clone(),
            Side::After,
            SplitOrientation::Horizontal,
        );
        assert!(
            result.is_err(),
            "splitting a cell with itself should return Err"
        );
    }

    #[test]
    fn split_with_nonexistent_target_leaves_store_intact() {
        let mut store = LayoutCellState::default();
        let a = new_root_pane(&mut store);
        let nonexistent = CellId::new();

        let before = snapshot(&store);
        let result = store.split_cell(nonexistent, a, Side::After, SplitOrientation::Horizontal);
        assert!(result.is_err(), "non-existent target should return Err");
        assert_eq!(
            snapshot(&store),
            before,
            "store should be unchanged when target is missing"
        );
    }

    #[test]
    fn split_with_nonexistent_new_cell_leaves_store_intact() {
        let mut store = LayoutCellState::default();
        let a = new_root_pane(&mut store);
        let nonexistent = CellId::new();

        let before = snapshot(&store);
        let result = store.split_cell(a, nonexistent, Side::After, SplitOrientation::Horizontal);
        assert!(result.is_err(), "non-existent new_cell should return Err");
        assert_eq!(
            snapshot(&store),
            before,
            "store should be unchanged when new_cell is missing"
        );
    }

    #[test]
    fn close_cell_rejects_split_target() {
        let mut store = LayoutCellState::default();
        let lhs = new_root_pane(&mut store);
        let rhs = new_root_pane(&mut store);
        let split_id = store
            .split_cell(
                lhs.clone(),
                rhs.clone(),
                Side::After,
                SplitOrientation::Horizontal,
            )
            .expect("split");

        let before = snapshot(&store);
        let result = store.close_cell(&split_id);
        assert!(
            matches!(result, Err(SessionError::InvalidCellType(ref id)) if id == &split_id),
            "closing a Split cell should return InvalidCellType, got {result:?}",
        );
        assert_eq!(
            snapshot(&store),
            before,
            "store must be unchanged when close rejects a Split target",
        );
    }

    #[test]
    fn close_cell_with_nonexistent_id_returns_err() {
        let mut store = LayoutCellState::default();
        let _ = new_root_pane(&mut store);
        let nonexistent = CellId::new();

        let before = snapshot(&store);
        let result = store.close_cell(&nonexistent);
        assert!(
            result.is_err(),
            "closing a nonexistent CellId should return Err"
        );
        assert_eq!(
            snapshot(&store),
            before,
            "store must be unchanged when close fails on nonexistent id",
        );
    }

    #[test]
    fn close_only_root_pane_returns_tree_emptied() {
        let mut store = LayoutCellState::default();
        let id = store.create_pane_cell(PaneId::new(), None);

        let outcome = store.close_cell(&id).expect("close should succeed");
        assert_eq!(outcome, CloseOutcome::TreeEmptied);
        assert_eq!(store.0.len(), 0);
        assert_well_formed(&store, None);
    }

    #[test]
    fn close_lhs_of_two_pane_root_split_replaces_root() {
        let mut store = LayoutCellState::default();
        let lhs = new_root_pane(&mut store);
        let rhs = new_root_pane(&mut store);
        let split_id = store
            .split_cell(
                lhs.clone(),
                rhs.clone(),
                Side::After,
                SplitOrientation::Horizontal,
            )
            .expect("split");

        let outcome = store.close_cell(&lhs).expect("close should succeed");
        assert_eq!(
            outcome,
            CloseOutcome::RootReplaced {
                new_root: rhs.clone()
            }
        );

        // Surviving sibling is the new root.
        assert_eq!(
            store.cell(&rhs).unwrap().parent,
            None,
            "rhs.parent must be None after promotion to root"
        );
        // Target and parent split are gone.
        assert!(store.cell(&lhs).is_err(), "closed pane should be removed");
        assert!(
            store.cell(&split_id).is_err(),
            "parent split should be removed"
        );
        assert_eq!(store.0.len(), 1);

        assert_well_formed(&store, Some(&rhs));
    }

    #[test]
    fn close_rhs_of_two_pane_root_split_replaces_root() {
        let mut store = LayoutCellState::default();
        let lhs = new_root_pane(&mut store);
        let rhs = new_root_pane(&mut store);
        let split_id = store
            .split_cell(
                lhs.clone(),
                rhs.clone(),
                Side::After,
                SplitOrientation::Horizontal,
            )
            .expect("split");

        let outcome = store.close_cell(&rhs).expect("close should succeed");
        assert_eq!(
            outcome,
            CloseOutcome::RootReplaced {
                new_root: lhs.clone()
            }
        );

        assert_eq!(store.cell(&lhs).unwrap().parent, None);
        assert!(store.cell(&rhs).is_err());
        assert!(store.cell(&split_id).is_err());
        assert_eq!(store.0.len(), 1);

        assert_well_formed(&store, Some(&lhs));
    }

    #[test]
    fn close_leaf_under_nested_split_promotes_sibling_to_grandparent_lhs_slot() {
        // Build:
        //                p_root (Split, root)
        //                /          \
        //              mid           b
        //            (Split)
        //            /    \
        //           a      c
        //
        // p_root.lhs = mid; p_root.rhs = b
        // mid.lhs = a; mid.rhs = c
        //
        // Close `a` → mid collapses; c is promoted into mid's slot in p_root (the lhs slot).
        // Expected: p_root.lhs = c, p_root.rhs = b unchanged. mid is removed. a is removed.
        //           c.parent = p_root.
        let mut store = LayoutCellState::default();
        let a = new_root_pane(&mut store);
        let b = new_root_pane(&mut store);
        let p_root = store
            .split_cell(
                a.clone(),
                b.clone(),
                Side::After,
                SplitOrientation::Horizontal,
            )
            .expect("first split");
        let c = new_root_pane(&mut store);
        let mid = store
            .split_cell(
                a.clone(),
                c.clone(),
                Side::After,
                SplitOrientation::Vertical,
            )
            .expect("second split");
        // Sanity check pre-state.
        assert_split(&store, &p_root, &mid, &b, SplitOrientation::Horizontal);
        assert_split(&store, &mid, &a, &c, SplitOrientation::Vertical);

        let outcome = store.close_cell(&a).expect("close should succeed");
        assert_eq!(
            outcome,
            CloseOutcome::SiblingPromoted {
                survivor: c.clone(),
                new_parent: p_root.clone(),
            }
        );

        // mid is removed; a is removed.
        assert!(store.cell(&a).is_err(), "a should be removed");
        assert!(store.cell(&mid).is_err(), "mid should be removed");

        // c.parent now points to p_root.
        assert_eq!(store.cell(&c).unwrap().parent.as_ref(), Some(&p_root));

        // p_root keeps its slot orientation: c sits in lhs (where mid was).
        assert_split(&store, &p_root, &c, &b, SplitOrientation::Horizontal);

        // b unchanged.
        assert_eq!(store.cell(&b).unwrap().parent.as_ref(), Some(&p_root));

        assert_well_formed(&store, Some(&p_root));
    }

    #[test]
    fn close_leaf_under_nested_split_promotes_sibling_to_grandparent_rhs_slot() {
        // Build:
        //                p_root (Split, root)
        //                /          \
        //               a           mid
        //                          (Split)
        //                          /    \
        //                         b      c
        //
        // Close `b` → mid collapses; c is promoted into mid's slot in p_root (the rhs slot).
        let mut store = LayoutCellState::default();
        let a = new_root_pane(&mut store);
        let b = new_root_pane(&mut store);
        let p_root = store
            .split_cell(
                a.clone(),
                b.clone(),
                Side::After,
                SplitOrientation::Horizontal,
            )
            .expect("first split");
        let c = new_root_pane(&mut store);
        let mid = store
            .split_cell(
                b.clone(),
                c.clone(),
                Side::After,
                SplitOrientation::Vertical,
            )
            .expect("second split");
        // Sanity.
        assert_split(&store, &p_root, &a, &mid, SplitOrientation::Horizontal);
        assert_split(&store, &mid, &b, &c, SplitOrientation::Vertical);

        let outcome = store.close_cell(&b).expect("close should succeed");
        assert_eq!(
            outcome,
            CloseOutcome::SiblingPromoted {
                survivor: c.clone(),
                new_parent: p_root.clone(),
            }
        );

        assert!(store.cell(&b).is_err());
        assert!(store.cell(&mid).is_err());
        assert_eq!(store.cell(&c).unwrap().parent.as_ref(), Some(&p_root));
        // c sits in rhs (where mid was).
        assert_split(&store, &p_root, &a, &c, SplitOrientation::Horizontal);
        assert_eq!(store.cell(&a).unwrap().parent.as_ref(), Some(&p_root));

        assert_well_formed(&store, Some(&p_root));
    }

    #[test]
    fn close_leaf_when_sibling_is_a_split_preserves_subtree() {
        // Build:
        //                p_root (Split, root)
        //                /          \
        //               a           mid
        //                          (Split)
        //                          /    \
        //                         b      c
        //
        // Close `a` → p_root collapses (it was target's parent and is the root).
        //              `mid` (the surviving sibling, itself a Split) becomes the new root.
        //              b and c stay parented to mid.
        let mut store = LayoutCellState::default();
        let a = new_root_pane(&mut store);
        let b = new_root_pane(&mut store);
        let p_root = store
            .split_cell(
                a.clone(),
                b.clone(),
                Side::After,
                SplitOrientation::Horizontal,
            )
            .expect("first split");
        let c = new_root_pane(&mut store);
        let mid = store
            .split_cell(
                b.clone(),
                c.clone(),
                Side::After,
                SplitOrientation::Vertical,
            )
            .expect("second split");
        assert_split(&store, &p_root, &a, &mid, SplitOrientation::Horizontal);

        let outcome = store.close_cell(&a).expect("close should succeed");
        assert_eq!(
            outcome,
            CloseOutcome::RootReplaced {
                new_root: mid.clone(),
            }
        );

        assert!(store.cell(&a).is_err());
        assert!(store.cell(&p_root).is_err());

        // mid is now the root.
        assert_eq!(store.cell(&mid).unwrap().parent, None);
        // mid's subtree (b, c) intact.
        assert_split(&store, &mid, &b, &c, SplitOrientation::Vertical);
        assert_eq!(store.cell(&b).unwrap().parent.as_ref(), Some(&mid));
        assert_eq!(store.cell(&c).unwrap().parent.as_ref(), Some(&mid));

        assert_well_formed(&store, Some(&mid));
    }

    #[test]
    fn close_cell_preserves_surviving_cell_ids() {
        // Verify that the surviving sibling's CellId value (and underlying LayoutCell.cell) is
        // preserved across close — only its `parent` field changes. This is the Pane.cell
        // back-pointer stability contract: PaneStore entries holding the surviving CellId
        // remain valid pointers into LayoutCellState after a close.
        let mut store = LayoutCellState::default();
        let lhs = new_root_pane(&mut store);
        let rhs = new_root_pane(&mut store);
        let _split_id = store
            .split_cell(
                lhs.clone(),
                rhs.clone(),
                Side::After,
                SplitOrientation::Horizontal,
            )
            .expect("split");

        // Capture the rhs cell's content before close.
        let rhs_cell_before = store.cell(&rhs).unwrap().cell.clone();

        let _ = store.close_cell(&lhs).expect("close should succeed");

        // CellId rhs still resolves; the underlying `cell` field (Pane variant + PaneId) is identical.
        let rhs_cell_after = store.cell(&rhs).unwrap();
        assert_eq!(
            rhs_cell_after.cell, rhs_cell_before,
            "surviving sibling's `cell` field must be byte-identical to pre-close",
        );
        // Only parent changed (Some(split_id) -> None for root case).
        assert_eq!(rhs_cell_after.parent, None);
    }

    #[test]
    fn split_orientation_serializes_lowercase() {
        assert_eq!(
            serde_json::to_string(&SplitOrientation::Horizontal).unwrap(),
            "\"horizontal\""
        );
        assert_eq!(
            serde_json::to_string(&SplitOrientation::Vertical).unwrap(),
            "\"vertical\""
        );
    }

    #[test]
    fn side_serializes_lowercase() {
        assert_eq!(serde_json::to_string(&Side::Before).unwrap(), "\"before\"");
        assert_eq!(serde_json::to_string(&Side::After).unwrap(), "\"after\"");
    }

    #[test]
    fn side_deserializes_lowercase() {
        let s: Side = serde_json::from_str("\"before\"").unwrap();
        assert_eq!(s, Side::Before);
    }

    #[test]
    fn layout_cell_state_serializes_as_object_keyed_by_cell_id() {
        let mut state = LayoutCellState::default();
        let pane_id = crate::pane::PaneId::new();
        let cell_id = state.create_pane_cell(pane_id.clone(), None);

        let v = serde_json::to_value(&state).unwrap();
        let obj = v.as_object().expect("object");
        assert!(obj.contains_key(cell_id.as_ref()));
    }
}
