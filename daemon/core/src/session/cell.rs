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
    pub fn get_pane_cell(&self, id: &CellId) -> OzmuxResult<&PaneCell> {
        match &self.cell(id)?.cell {
            Cell::Pane(pane) => Ok(pane),
            _ => Err(OzmuxError::InvalidCellType(id.clone())),
        }
    }

    #[inline]
    pub fn get_split_cell(&self, id: &CellId) -> OzmuxResult<&SplitCell> {
        match &self.cell(id)?.cell {
            Cell::Split(split) => Ok(split),
            _ => Err(OzmuxError::InvalidCellType(id.clone())),
        }
    }

    pub fn split_cell(
        &mut self,
        target: CellId,
        new_cell: CellId,
        new_cell_side: Side,
        orientation: SplitOrientation,
    ) -> OzmuxResult<CellId> {
        if target == new_cell {
            return Err(OzmuxError::SplitTargetEqualsNewCell(target));
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

        self.replace_child_to_split_cell(&target, target_parent.clone(), split_cell_id.clone())?;

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
    /// # Algorithm
    /// 1. **Pre-validate (no mutation)**: target exists and is `Cell::Pane`;
    ///    if non-root, parent exists and is `Cell::Split`; sibling found via
    ///    `obtain_other_side_cell_id`; grandparent (if any) exists and is `Cell::Split`.
    ///    Any failure → `Err`, store unchanged.
    /// 2. **Commit (atomic, fixed order)**: remove target; remove parent split (if any);
    ///    set sibling.parent = grandparent_id_or_None; rewrite grandparent's child
    ///    slot from parent_id to sibling_id (preserving lhs/rhs orientation).
    ///
    /// # Discipline rule (reviewers must enforce)
    /// All fallible reads (`?`-returning) MUST appear before any `remove`/`cell_mut`.
    /// The commit phase is infallible by construction. Rust's type system does not
    /// enforce this in-function ordering.
    ///
    /// # Identifier stability
    /// Only `id` (and the parent split, if any) are removed from the store.
    /// All other `CellId`s remain valid; the only `LayoutCell` field updated besides
    /// removals is the promoted sibling's `parent` (and the grandparent's child slot).
    pub fn close_cell(&mut self, id: &CellId) -> OzmuxResult<CloseOutcome> {
        // ─── Pre-validate phase: no mutation ───
        let target = self.cell(id)?;
        let Cell::Pane(_) = &target.cell else {
            return Err(OzmuxError::InvalidCellType(id.clone()));
        };
        let parent_id = target.parent.clone();

        let Some(parent_id) = parent_id else {
            // Target is the root pane (parentless). Commit phase trivial.
            self.0.remove(id);
            return Ok(CloseOutcome::TreeEmptied);
        };

        let parent = self.cell(&parent_id)?;
        let Cell::Split(parent_split) = &parent.cell else {
            return Err(OzmuxError::InvalidCellType(parent_id));
        };
        let sibling_id = parent_split.obtain_other_side_cell_id(id).clone();
        let grandparent_id = parent.parent.clone();

        if let Some(gp_id) = grandparent_id.as_ref() {
            let grandparent = self.cell(gp_id)?;
            let Cell::Split(_) = &grandparent.cell else {
                return Err(OzmuxError::InvalidCellType(gp_id.clone()));
            };
        }

        // ─── Commit phase: all checks passed; mutations are infallible ───
        self.0.remove(id);
        self.0
            .remove(&parent_id)
            .expect("parent existed in pre-validate");
        let sibling = self
            .0
            .get_mut(&sibling_id)
            .expect("sibling existed in pre-validate");
        sibling.parent = grandparent_id.clone();

        match grandparent_id {
            Some(gp_id) => {
                let grandparent = self
                    .0
                    .get_mut(&gp_id)
                    .expect("grandparent existed in pre-validate");
                let Cell::Split(grandparent_split) = &mut grandparent.cell else {
                    unreachable!("grandparent type checked in pre-validate phase");
                };
                if grandparent_split.lhs_cell == parent_id {
                    grandparent_split.lhs_cell = sibling_id.clone();
                } else if grandparent_split.rhs_cell == parent_id {
                    grandparent_split.rhs_cell = sibling_id.clone();
                } else {
                    unreachable!(
                        "grandparent referenced parent at lhs or rhs in pre-validate phase"
                    );
                }
                Ok(CloseOutcome::SiblingPromoted {
                    survivor: sibling_id,
                    new_parent: gp_id,
                })
            }
            None => Ok(CloseOutcome::RootReplaced {
                new_root: sibling_id,
            }),
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
    pub fn cell(&self, id: &CellId) -> OzmuxResult<&LayoutCell> {
        self.0
            .get(id)
            .ok_or_else(|| OzmuxError::CellNotfound(id.clone()))
    }

    #[inline]
    fn cell_mut(&mut self, id: &CellId) -> OzmuxResult<&mut LayoutCell> {
        self.0
            .get_mut(id)
            .ok_or_else(|| OzmuxError::CellNotfound(id.clone()))
    }

    #[inline]
    fn remove(&mut self, id: &CellId) -> OzmuxResult<LayoutCell> {
        self.0
            .remove(id)
            .ok_or_else(|| OzmuxError::CellNotfound(id.clone()))
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

    pub fn obtain_other_side_cell_id(&self, id: &CellId) -> &CellId {
        if &self.lhs_cell == id {
            &self.rhs_cell
        } else {
            &self.lhs_cell
        }
    }
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub enum SplitOrientation {
    Vertical,
    Horizontal,
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    /// Place new_cell before target (left or top, depending on orientation).
    Before,
    /// Place new_cell after target (right or bottom, depending on orientation).
    After,
}

/// Structural outcome of `LayoutCellStore::close_cell`.
///
/// Callers (typically `SessionStore::close_pane`) must handle every variant.
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
    use crate::session::pane::PaneId;
    use std::collections::{HashMap, HashSet};

    /// Create a parent-less pane cell, insert it into the store, and return its CellId.
    /// Equivalent to a freshly-created tmux pane (the kind passed as new_cell to split_cell).
    fn new_root_pane(store: &mut LayoutCellStore) -> CellId {
        let pane_id = PaneId::new();
        store.create_pane_cell(pane_id, None)
    }

    /// Snapshot the store's internal HashMap by full clone.
    /// Used for "store unchanged" assertions in failure-path tests.
    fn snapshot(store: &LayoutCellStore) -> HashMap<CellId, LayoutCell> {
        store.0.clone()
    }

    /// Assert the cell is a Split with the expected (lhs_cell, rhs_cell, orientation),
    /// and that lhs_weight == rhs_weight == 0.5 (the default produced by SplitCell::new).
    fn assert_split(
        store: &LayoutCellStore,
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
    fn assert_well_formed(store: &LayoutCellStore, root: Option<&CellId>) {
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
        let store = LayoutCellStore::default();
        assert_well_formed(&store, None);
        let _snap = snapshot(&store);
    }

    #[test]
    fn split_root_pane() {
        let mut store = LayoutCellStore::default();
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
        let mut store = LayoutCellStore::default();
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
        let mut store = LayoutCellStore::default();
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
        let mut store = LayoutCellStore::default();
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
        let mut store = LayoutCellStore::default();
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
        let mut store = LayoutCellStore::default();
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
        let mut store = LayoutCellStore::default();
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
        let mut store = LayoutCellStore::default();
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
            matches!(result, Err(OzmuxError::InvalidCellType(ref id)) if id == &split_id),
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
        let mut store = LayoutCellStore::default();
        let _ = new_root_pane(&mut store);
        let nonexistent = CellId::new();

        let before = snapshot(&store);
        let result = store.close_cell(&nonexistent);
        assert!(result.is_err(), "closing a nonexistent CellId should return Err");
        assert_eq!(
            snapshot(&store),
            before,
            "store must be unchanged when close fails on nonexistent id",
        );
    }

    #[test]
    fn close_only_root_pane_returns_tree_emptied() {
        let mut store = LayoutCellStore::default();
        let id = store.create_pane_cell(PaneId::new(), None);

        let outcome = store.close_cell(&id).expect("close should succeed");
        assert_eq!(outcome, CloseOutcome::TreeEmptied);
        assert_eq!(store.0.len(), 0);
        assert_well_formed(&store, None);
    }

    #[test]
    fn close_lhs_of_two_pane_root_split_replaces_root() {
        let mut store = LayoutCellStore::default();
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
        assert!(
            store.cell(&lhs).is_err(),
            "closed pane should be removed"
        );
        assert!(
            store.cell(&split_id).is_err(),
            "parent split should be removed"
        );
        assert_eq!(store.0.len(), 1);

        assert_well_formed(&store, Some(&rhs));
    }

    #[test]
    fn close_rhs_of_two_pane_root_split_replaces_root() {
        let mut store = LayoutCellStore::default();
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
        let mut store = LayoutCellStore::default();
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
        let mut store = LayoutCellStore::default();
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
        let mut store = LayoutCellStore::default();
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
        // remain valid pointers into LayoutCellStore after a close.
        let mut store = LayoutCellStore::default();
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
}
