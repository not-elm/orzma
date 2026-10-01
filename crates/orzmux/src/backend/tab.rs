//! The ordered set of tabs: each tiles its own layout tree, and one
//! of them is displayed.

use crate::backend::layout::{LayoutTree, Removal, SplitIds};
use crate::backend::{CloseTarget, PaneId, SplitOrientation, TabEntry, TabId, TabTarget};
use crate::error::{OrzmuxError, OrzmuxResult};
use orzma_vt::prelude::GridSize;

/// One tab: its name and the tree its panes tile.
#[derive(Debug)]
pub struct Tab {
    /// The tab's id.
    pub id: TabId,
    /// The name the user gave it, or `None` for the automatic name.
    pub name: Option<String>,
    /// The panes it tiles.
    pub tree: LayoutTree,
}

impl Tab {
    /// The longest tab name kept, in `char`s.
    pub const MAX_NAME_CHARS: usize = 64;
}

/// What [`Tabs::remove_pane`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneRemoval {
    /// No tab holds the pane.
    Absent,
    /// The pane left its tab, which keeps its other panes.
    Removed,
    /// The pane was its tab's only pane, and the tab was removed with it.
    TabClosed(TabId),
}

/// Every tab in display order, the displayed one, and the split-id
/// minter their trees share.
///
/// # Invariants
///
/// The displayed id names a tab of the set whenever the set is not
/// empty, and is `None` when it is empty. Tab ids are never reused.
#[derive(Debug, Default)]
pub struct Tabs {
    order: Vec<Tab>,
    active: Option<TabId>,
    last_id: u32,
    split_ids: SplitIds,
}

impl Tabs {
    /// Appends a tab whose only pane is `root` and returns its id.
    /// It becomes displayed only when it is the only tab.
    pub fn create(&mut self, root: PaneId) -> TabId {
        self.last_id += 1;
        let id = TabId(self.last_id);
        self.order.push(Tab {
            id,
            name: None,
            tree: LayoutTree::with_root(root),
        });
        if self.active.is_none() {
            self.active = Some(id);
        }
        id
    }

    /// Removes `pane` from the tab that holds it. A tab whose only pane it
    /// was is removed as a whole; when it was displayed, the tab that takes
    /// its position is displayed, or the new last one when it was last.
    pub fn remove_pane(&mut self, pane: PaneId) -> PaneRemoval {
        let Some(index) = self.order.iter().position(|t| t.tree.contains(pane)) else {
            return PaneRemoval::Absent;
        };
        let Tab { id, name, tree } = self.order.remove(index);
        match tree.remove(pane) {
            Removal::Removed(tree) => {
                self.order.insert(index, Tab { id, name, tree });
                PaneRemoval::Removed
            }
            Removal::Absent(tree) => {
                self.order.insert(index, Tab { id, name, tree });
                PaneRemoval::Absent
            }
            Removal::Last(_) => {
                if self.active == Some(id) {
                    self.active = self
                        .order
                        .get(index)
                        .or_else(|| self.order.last())
                        .map(|t| t.id);
                }
                PaneRemoval::TabClosed(id)
            }
        }
    }

    /// Displays `id`. Returns whether the displayed tab changed; an
    /// unknown id changes nothing.
    pub fn activate(&mut self, id: TabId) -> bool {
        if self.active == Some(id) || self.index_of(id).is_none() {
            return false;
        }
        self.active = Some(id);
        true
    }

    /// The tab `target` names in the current order, or `None` when
    /// it names none.
    pub fn resolve(&self, target: TabTarget) -> Option<TabId> {
        let displayed = self.active.and_then(|id| self.index_of(id));
        let len = self.order.len();
        let index = match target {
            TabTarget::Active => displayed?,
            TabTarget::Id(id) => self.index_of(id)?,
            TabTarget::Index(index) => usize::from(index),
            TabTarget::Next => (displayed? + 1) % len,
            TabTarget::Previous => (displayed? + len - 1) % len,
        };
        self.order.get(index).map(|tab| tab.id)
    }

    /// The tab `target` names, or `None` when it names none.
    pub fn resolve_close(&self, target: CloseTarget) -> Option<TabId> {
        match target {
            CloseTarget::Active => self.active,
            CloseTarget::Id(id) => self.index_of(id).map(|_| id),
        }
    }

    /// Moves `id` to the zero-based `index`, clamped to the last position.
    /// Returns whether the order changed.
    ///
    /// # Errors
    ///
    /// Returns [`OrzmuxError::UnresolvedTab`] for an unknown id;
    /// the order is unchanged.
    pub fn move_to(&mut self, id: TabId, index: u16) -> OrzmuxResult<bool> {
        let from = self.index_of(id).ok_or(OrzmuxError::UnresolvedTab)?;
        let to = usize::from(index).min(self.order.len().saturating_sub(1));
        if from == to {
            return Ok(false);
        }
        let tab = self.order.remove(from);
        self.order.insert(to, tab);
        Ok(true)
    }

    /// Names `id` after `name` with control characters removed, the
    /// surrounding whitespace trimmed, and at most
    /// [`Tab::MAX_NAME_CHARS`] characters kept. A name that ends up
    /// empty, or `None`, restores the automatic name. Returns whether the
    /// name changed.
    ///
    /// # Errors
    ///
    /// Returns [`OrzmuxError::UnresolvedTab`] for an unknown id.
    pub fn rename(&mut self, id: TabId, name: Option<String>) -> OrzmuxResult<bool> {
        let tab = self.get_mut(id).ok_or(OrzmuxError::UnresolvedTab)?;
        let name = name.as_deref().and_then(sanitized_name);
        if tab.name == name {
            return Ok(false);
        }
        tab.name = name;
        Ok(true)
    }

    /// Splits `target`, a pane of the displayed tab, placing `new`
    /// right of / below it.
    ///
    /// # Errors
    ///
    /// Returns [`OrzmuxError::UnresolvedTarget`] when `target` is not in
    /// the displayed tab, and [`OrzmuxError::SplitRefused`] when the
    /// tree refuses the split.
    pub fn split_active(
        &mut self,
        target: PaneId,
        orientation: SplitOrientation,
        new: PaneId,
        window: GridSize,
    ) -> OrzmuxResult {
        let active = self.active.ok_or(OrzmuxError::UnresolvedTarget)?;
        let tab = self
            .order
            .iter_mut()
            .find(|tab| tab.id == active)
            .ok_or(OrzmuxError::UnresolvedTarget)?;
        if !tab.tree.contains(target) {
            return Err(OrzmuxError::UnresolvedTarget);
        }
        tab.tree
            .split(&mut self.split_ids, target, orientation, new, window)
    }

    /// The displayed tab's id.
    pub fn active_id(&self) -> Option<TabId> {
        self.active
    }

    /// The displayed tab.
    pub fn active(&self) -> Option<&Tab> {
        self.active.and_then(|id| self.get(id))
    }

    /// The displayed tab, for a change to its tree.
    pub fn active_mut(&mut self) -> Option<&mut Tab> {
        let id = self.active?;
        self.get_mut(id)
    }

    /// The tab `id` names.
    pub fn get(&self, id: TabId) -> Option<&Tab> {
        self.order.iter().find(|tab| tab.id == id)
    }

    /// The tab `id` names, for a change.
    pub fn get_mut(&mut self, id: TabId) -> Option<&mut Tab> {
        self.order.iter_mut().find(|tab| tab.id == id)
    }

    /// The tab whose tree holds `pane`.
    #[cfg(test)]
    pub fn tab_of(&self, pane: PaneId) -> Option<TabId> {
        self.order
            .iter()
            .find(|tab| tab.tree.contains(pane))
            .map(|tab| tab.id)
    }

    /// Every tab in display order.
    pub fn iter(&self) -> impl Iterator<Item = &Tab> {
        self.order.iter()
    }

    /// Every tab in display order, as the GUI lists them.
    pub fn entries(&self) -> Vec<TabEntry> {
        self.order
            .iter()
            .map(|tab| TabEntry {
                id: tab.id,
                name: tab.name.clone(),
                active_pane: tab.tree.active(),
            })
            .collect()
    }

    /// Whether `entries` and `active` describe the tabs as they are now:
    /// the same tabs in the same order, with the same names and active
    /// panes, and the same displayed tab.
    pub fn lists_as(&self, entries: &[TabEntry], active: Option<TabId>) -> bool {
        self.active == active
            && self.order.len() == entries.len()
            && self.order.iter().zip(entries).all(|(tab, entry)| {
                let TabEntry {
                    id,
                    name,
                    active_pane,
                } = entry;
                tab.id == *id && tab.name == *name && tab.tree.active() == *active_pane
            })
    }

    fn index_of(&self, id: TabId) -> Option<usize> {
        self.order.iter().position(|tab| tab.id == id)
    }
}

/// `name` without control characters, trimmed, and cut to
/// [`Tab::MAX_NAME_CHARS`] characters; `None` when nothing is left.
fn sanitized_name(name: &str) -> Option<String> {
    let kept: String = name.chars().filter(|c| !c.is_control()).collect();
    let cut: String = kept.trim().chars().take(Tab::MAX_NAME_CHARS).collect();
    let trimmed = cut.trim_end();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: GridSize = GridSize { cols: 80, rows: 24 };

    fn three() -> (Tabs, [TabId; 3]) {
        let mut set = Tabs::default();
        let a = set.create(PaneId(1));
        let b = set.create(PaneId(2));
        let c = set.create(PaneId(3));
        (set, [a, b, c])
    }

    /// Asserts that only the first tab becomes displayed on creation
    /// and that ids increase.
    ///
    /// Case: orzma starts with one tab, then the user opens two more
    /// in the background of the spawn.
    #[test]
    fn the_first_tab_is_displayed_and_ids_increase() {
        let (set, [a, b, c]) = three();
        assert_eq!(set.active_id(), Some(a));
        assert!(a < b && b < c);
        assert_eq!(set.entries().len(), 3);
    }

    /// Asserts that removing a tab's only pane removes the tab and
    /// displays the tab that takes its position, or the new last one when
    /// it was last.
    ///
    /// Case: the user closes the displayed tab in the middle, then the
    /// last tab, each holding one shell.
    #[test]
    fn removing_a_tabs_only_pane_closes_it_and_displays_its_right_neighbour() {
        let (mut set, [a, b, c]) = three();
        set.activate(b);
        assert_eq!(set.remove_pane(PaneId(2)), PaneRemoval::TabClosed(b));
        assert_eq!(set.active_id(), Some(c));
        assert_eq!(set.remove_pane(PaneId(3)), PaneRemoval::TabClosed(c));
        assert_eq!(set.active_id(), Some(a));
        assert_eq!(set.remove_pane(PaneId(1)), PaneRemoval::TabClosed(a));
        assert_eq!(set.active_id(), None);
        assert!(set.entries().is_empty());
    }

    /// Asserts that removing one of a tab's two panes keeps the tab in
    /// its place with the other pane active.
    ///
    /// Case: the user splits the first tab, then the new pane's shell
    /// exits.
    #[test]
    fn removing_one_of_two_panes_keeps_the_tab() {
        let (mut set, [a, b, c]) = three();
        set.split_active(PaneId(1), SplitOrientation::Vertical, PaneId(10), W)
            .expect("the displayed root splits");
        assert_eq!(set.remove_pane(PaneId(10)), PaneRemoval::Removed);
        let order: Vec<_> = set.entries().into_iter().map(|e| e.id).collect();
        assert_eq!(order, vec![a, b, c]);
        let tree = &set.get(a).expect("the first tab stays").tree;
        assert_eq!(tree.panes(), vec![PaneId(1)]);
        assert_eq!(tree.active(), PaneId(1));
        assert_eq!(set.active_id(), Some(a));
    }

    /// Asserts that removing a pane no tab holds changes nothing.
    ///
    /// Case: a stale close names a pane that already left its tab.
    #[test]
    fn removing_an_unknown_pane_changes_nothing() {
        let (mut set, _) = three();
        assert_eq!(set.remove_pane(PaneId(99)), PaneRemoval::Absent);
        assert_eq!(set.entries().len(), 3);
    }

    /// Asserts that `Next` and `Previous` wrap and that an out-of-range
    /// index resolves to nothing.
    ///
    /// Case: the user cycles past the last tab and presses the key for a
    /// ninth tab that does not exist.
    #[test]
    fn targets_wrap_and_out_of_range_indexes_resolve_to_nothing() {
        let (mut set, [a, _b, c]) = three();
        set.activate(c);
        assert_eq!(set.resolve(TabTarget::Next), Some(a));
        set.activate(a);
        assert_eq!(set.resolve(TabTarget::Previous), Some(c));
        assert_eq!(set.resolve(TabTarget::Index(2)), Some(c));
        assert_eq!(set.resolve(TabTarget::Index(8)), None);
    }

    /// Asserts that a move clamps to the last position, reports whether the
    /// order changed, and refuses an unknown tab.
    ///
    /// Case: the user drags the first tab past the end of the bar, then
    /// drops a tab where it already was.
    #[test]
    fn a_move_clamps_and_reports_whether_the_order_changed() {
        let (mut set, [a, b, c]) = three();
        assert_eq!(set.move_to(a, 99).ok(), Some(true));
        let order: Vec<_> = set.entries().into_iter().map(|e| e.id).collect();
        assert_eq!(order, vec![b, c, a]);
        assert_eq!(set.move_to(a, 2).ok(), Some(false));
        assert!(matches!(
            set.move_to(TabId(99), 0),
            Err(OrzmuxError::UnresolvedTab)
        ));
    }

    /// Asserts that a rename strips control characters, trims, keeps at
    /// most 64 characters, turns an empty result into the automatic name,
    /// and reports whether the name changed.
    ///
    /// Case: the user pastes a name with a tab and a newline into the
    /// rename field, then clears the field.
    #[test]
    fn a_rename_is_sanitized_and_reports_a_change() {
        let (mut set, [a, ..]) = three();
        assert_eq!(set.rename(a, Some("  lo\tgs\n ".into())).ok(), Some(true));
        assert_eq!(
            set.get(a).and_then(|tab| tab.name.clone()),
            Some("logs".into())
        );
        assert_eq!(set.rename(a, Some("logs".into())).ok(), Some(false));
        let long: String = "あ".repeat(70);
        set.rename(a, Some(long)).expect("a known tab");
        assert_eq!(
            set.get(a)
                .and_then(|tab| tab.name.clone())
                .map(|n| n.chars().count()),
            Some(Tab::MAX_NAME_CHARS)
        );
        assert_eq!(set.rename(a, Some("   ".into())).ok(), Some(true));
        assert_eq!(set.get(a).and_then(|tab| tab.name.clone()), None);
    }

    /// Asserts that a split must target the displayed tab and that
    /// split ids stay unique across tabs.
    ///
    /// Case: the user splits a pane in one tab, switches, and splits
    /// in the other, while a stale request still names a hidden pane.
    #[test]
    fn splits_target_the_displayed_tab_with_unique_ids() {
        let (mut set, [a, b, _c]) = three();
        set.split_active(PaneId(1), SplitOrientation::Vertical, PaneId(10), W)
            .expect("the displayed root splits");
        assert!(matches!(
            set.split_active(PaneId(2), SplitOrientation::Vertical, PaneId(11), W),
            Err(OrzmuxError::UnresolvedTarget)
        ));
        set.activate(b);
        set.split_active(PaneId(2), SplitOrientation::Vertical, PaneId(12), W)
            .expect("the displayed root splits");
        let split_of = |id| {
            set.get(id)
                .map(|tab| tab.tree.tile(W).separators[0].split)
                .expect("a split tab")
        };
        assert_ne!(split_of(a), split_of(b));
        assert_eq!(set.tab_of(PaneId(12)), Some(b));
    }
}
