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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::pane::PaneId;
    use std::collections::HashMap;

    /// parent: None の Pane Cell を作成して store に挿入し、その CellId を返す。
    /// tmux における「新規ペイン」(create_split_cell の rhs 引数として渡す対象) に相当。
    fn new_root_pane(store: &mut LayoutCellStore) -> CellId {
        let pane_id = PaneId::new();
        store.create_pane_cell(pane_id, None)
    }

    /// 失敗系テスト用: store の内部 HashMap を完全クローンしてスナップショットする。
    fn snapshot(store: &LayoutCellStore) -> HashMap<CellId, LayoutCell> {
        store.0.clone()
    }

    /// 指定セルが Split で、期待する (lhs_cell, rhs_cell, orientation) を持ち、
    /// かつ lhs_weight == rhs_weight == 0.5 (SplitCell::new の初期値) であることを検証。
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

    /// store 全体について木の整合性を検証:
    ///   (a) 各セルの parent が Some(p) なら p は store に存在し Cell::Split で、
    ///       p.lhs_cell または p.rhs_cell が自身を指す
    ///   (b) 各 Split の lhs_cell/rhs_cell は store に存在し、その parent が自身を指す
    ///   (c) 各非ルートセルは store 全体の Split のうちちょうど 1 回だけ子参照される
    ///       (重複参照・ぶら下がり参照を検出)
    fn assert_well_formed(store: &LayoutCellStore) {
        for (id, cell) in &store.0 {
            // (a) parent backref
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

            // (b) Split children forward consistency
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

        // (c) each non-root cell referenced exactly once across all Splits
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
    }

    #[test]
    fn helpers_compile_smoke() {
        // ヘルパーが少なくともコンパイルし、空ストアでもエラーを起こさないことを確認。
        let store = LayoutCellStore::default();
        assert_well_formed(&store);
        let _snap = snapshot(&store);
    }

    #[test]
    fn split_root_pane() {
        let mut store = LayoutCellStore::default();
        let lhs = new_root_pane(&mut store);
        let rhs = new_root_pane(&mut store);

        // 分割前: 両者とも parent: None
        assert_eq!(store.cell(&lhs).unwrap().parent, None);
        assert_eq!(store.cell(&rhs).unwrap().parent, None);

        let split_id = store
            .create_split_cell(lhs.clone(), rhs.clone(), SplitOrientation::Horizontal)
            .expect("split should succeed");

        // 新 split がルートになる
        assert_eq!(
            store.cell(&split_id).unwrap().parent,
            None,
            "new split should be root"
        );

        // (b) 契約の本丸: rhs.parent が None → split_id へ遷移
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
        assert_well_formed(&store);
    }

    #[test]
    fn split_lhs_position_of_existing_split() {
        let mut store = LayoutCellStore::default();
        let a = new_root_pane(&mut store);
        let b = new_root_pane(&mut store);
        // 初期 split: P{lhs=A, rhs=B}
        let p = store
            .create_split_cell(a.clone(), b.clone(), SplitOrientation::Horizontal)
            .expect("first split");

        // P の lhs 側 (= A) を分割する
        let new_pane = new_root_pane(&mut store);
        let new_split = store
            .create_split_cell(a.clone(), new_pane.clone(), SplitOrientation::Vertical)
            .expect("second split");

        // P.lhs_cell が new_split に差し替わり、P.rhs_cell は B のまま
        assert_split(&store, &p, &new_split, &b, SplitOrientation::Horizontal);

        // new_split の構造
        assert_split(
            &store,
            &new_split,
            &a,
            &new_pane,
            SplitOrientation::Vertical,
        );

        // 親子ポインタ
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

        // B は不変 (parent は P のまま)
        assert_eq!(
            store.cell(&b).unwrap().parent.as_ref(),
            Some(&p),
            "B should be unchanged"
        );

        assert_well_formed(&store);
    }

    #[test]
    fn split_rhs_position_of_existing_split() {
        let mut store = LayoutCellStore::default();
        let a = new_root_pane(&mut store);
        let b = new_root_pane(&mut store);
        // 初期 split: P{lhs=A, rhs=B}
        let p = store
            .create_split_cell(a.clone(), b.clone(), SplitOrientation::Horizontal)
            .expect("first split");

        // P の rhs 側 (= B) を分割する
        let new_pane = new_root_pane(&mut store);
        let new_split = store
            .create_split_cell(b.clone(), new_pane.clone(), SplitOrientation::Vertical)
            .expect("second split");

        // P.rhs_cell が new_split に差し替わり、P.lhs_cell は A のまま
        assert_split(&store, &p, &a, &new_split, SplitOrientation::Horizontal);

        // new_split の構造
        assert_split(
            &store,
            &new_split,
            &b,
            &new_pane,
            SplitOrientation::Vertical,
        );

        // 親子ポインタ
        assert_eq!(store.cell(&new_split).unwrap().parent.as_ref(), Some(&p));
        assert_eq!(store.cell(&p).unwrap().parent, None);
        assert_eq!(store.cell(&b).unwrap().parent.as_ref(), Some(&new_split));
        assert_eq!(
            store.cell(&new_pane).unwrap().parent.as_ref(),
            Some(&new_split)
        );

        // A は不変 (parent は P のまま)
        assert_eq!(
            store.cell(&a).unwrap().parent.as_ref(),
            Some(&p),
            "A should be unchanged"
        );

        assert_well_formed(&store);
    }

    #[test]
    fn split_twice_deeply() {
        let mut store = LayoutCellStore::default();
        let a = new_root_pane(&mut store);
        let b = new_root_pane(&mut store);
        // p_root{lhs=A, rhs=B}
        let p_root = store
            .create_split_cell(a.clone(), b.clone(), SplitOrientation::Horizontal)
            .unwrap();

        // A をさらに分割: p_root{lhs=mid{lhs=A, rhs=C}, rhs=B}
        let c = new_root_pane(&mut store);
        let mid = store
            .create_split_cell(a.clone(), c.clone(), SplitOrientation::Vertical)
            .unwrap();

        // A をもう一度分割: p_root{lhs=mid{lhs=inner{lhs=A, rhs=D}, rhs=C}, rhs=B}
        let d = new_root_pane(&mut store);
        let inner = store
            .create_split_cell(a.clone(), d.clone(), SplitOrientation::Horizontal)
            .unwrap();

        // 各 split の構造
        assert_split(&store, &p_root, &mid, &b, SplitOrientation::Horizontal);
        assert_split(&store, &mid, &inner, &c, SplitOrientation::Vertical);
        assert_split(&store, &inner, &a, &d, SplitOrientation::Horizontal);

        // 祖先チェーンが正しく繋がっている
        assert_eq!(store.cell(&p_root).unwrap().parent, None);
        assert_eq!(store.cell(&mid).unwrap().parent.as_ref(), Some(&p_root));
        assert_eq!(store.cell(&inner).unwrap().parent.as_ref(), Some(&mid));
        assert_eq!(store.cell(&a).unwrap().parent.as_ref(), Some(&inner));

        // 同レベルの兄弟が壊れていない
        assert_eq!(store.cell(&b).unwrap().parent.as_ref(), Some(&p_root));
        assert_eq!(store.cell(&c).unwrap().parent.as_ref(), Some(&mid));
        assert_eq!(store.cell(&d).unwrap().parent.as_ref(), Some(&inner));

        assert_well_formed(&store);
    }

    // TODO(phase-2): 現コードは lhs == rhs を検証せず Ok を返し、SplitCell が
    // 同じ ID を lhs_cell/rhs_cell の両方に持つ縮退状態になる。
    // Phase 2 で create_split_cell に同一性検証を追加し、Err 化する。
    #[test]
    fn split_with_same_lhs_rhs_returns_error() {
        let mut store = LayoutCellStore::default();
        let a = new_root_pane(&mut store);

        let result = store.create_split_cell(a.clone(), a.clone(), SplitOrientation::Horizontal);
        assert!(
            result.is_err(),
            "splitting a cell with itself should return Err"
        );
    }

    #[test]
    fn split_with_nonexistent_lhs_leaves_store_intact() {
        let mut store = LayoutCellStore::default();
        let a = new_root_pane(&mut store);
        let nonexistent = CellId::new();

        let before = snapshot(&store);
        let result = store.create_split_cell(nonexistent, a, SplitOrientation::Horizontal);
        assert!(result.is_err(), "non-existent lhs should return Err");
        assert_eq!(
            snapshot(&store),
            before,
            "store should be unchanged when lhs is missing"
        );
    }

    #[test]
    fn split_with_nonexistent_rhs_leaves_store_intact() {
        // 現コードは parent(&lhs)? / parent(&rhs)? の両方の existence check が
        // mutation より先に走るため、rhs 不在時もアトミック (lhs.parent は更新されない)。
        // 回帰検出用テスト。
        let mut store = LayoutCellStore::default();
        let a = new_root_pane(&mut store);
        let nonexistent = CellId::new();

        let before = snapshot(&store);
        let result = store.create_split_cell(a, nonexistent, SplitOrientation::Horizontal);
        assert!(result.is_err(), "non-existent rhs should return Err");
        assert_eq!(
            snapshot(&store),
            before,
            "store should be unchanged when rhs is missing"
        );
    }
}
