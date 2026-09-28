//! The ordered set of workspaces: each tiles its own layout tree, and one
//! of them is displayed.

use crate::backend::layout::{LayoutTree, SplitIds};
use crate::backend::{
    CloseTarget, PaneId, SplitOrientation, WorkspaceEntry, WorkspaceId, WorkspaceTarget,
};
use crate::error::{OrzmuxError, OrzmuxResult};
use orzma_vt::prelude::GridSize;

/// The longest workspace name kept, in `char`s.
pub const MAX_NAME_CHARS: usize = 64;

/// One workspace: its name and the tree its panes tile.
#[derive(Debug)]
pub struct Workspace {
    /// The workspace's id.
    pub id: WorkspaceId,
    /// The name the user gave it, or `None` for the automatic name.
    pub name: Option<String>,
    /// The panes it tiles.
    pub tree: LayoutTree,
}

/// Every workspace in display order, the displayed one, and the split-id
/// minter their trees share.
///
/// # Invariants
///
/// The displayed id names a workspace of the set whenever the set is not
/// empty, and is `None` when it is empty. Workspace ids are never reused.
#[derive(Debug, Default)]
pub struct Workspaces {
    order: Vec<Workspace>,
    active: Option<WorkspaceId>,
    last_id: u32,
    split_ids: SplitIds,
}

impl Workspaces {
    /// Appends a workspace whose only pane is `root` and returns its id.
    /// It becomes displayed only when it is the only workspace.
    pub fn create(&mut self, root: PaneId) -> WorkspaceId {
        self.last_id += 1;
        let id = WorkspaceId(self.last_id);
        self.order.push(Workspace {
            id,
            name: None,
            tree: LayoutTree::with_root(root),
        });
        if self.active.is_none() {
            self.active = Some(id);
        }
        id
    }

    /// Removes `id`. When it was displayed, the workspace that takes its
    /// position is displayed, or the new last one when it was last.
    /// Returns whether it existed.
    pub fn remove(&mut self, id: WorkspaceId) -> bool {
        let Some(index) = self.index_of(id) else {
            return false;
        };
        self.order.remove(index);
        if self.active == Some(id) {
            self.active = self
                .order
                .get(index)
                .or_else(|| self.order.last())
                .map(|w| w.id);
        }
        true
    }

    /// Displays `id`. Returns whether the displayed workspace changed; an
    /// unknown id changes nothing.
    pub fn activate(&mut self, id: WorkspaceId) -> bool {
        if self.active == Some(id) || self.index_of(id).is_none() {
            return false;
        }
        self.active = Some(id);
        true
    }

    /// The workspace `target` names in the current order, or `None` when
    /// it names none.
    pub fn resolve(&self, target: WorkspaceTarget) -> Option<WorkspaceId> {
        let displayed = self.active.and_then(|id| self.index_of(id));
        let len = self.order.len();
        let index = match target {
            WorkspaceTarget::Active => displayed?,
            WorkspaceTarget::Id(id) => self.index_of(id)?,
            WorkspaceTarget::Index(index) => usize::from(index),
            WorkspaceTarget::Next => (displayed? + 1) % len,
            WorkspaceTarget::Previous => (displayed? + len - 1) % len,
        };
        self.order.get(index).map(|w| w.id)
    }

    /// The workspace `target` names, or `None` when it names none.
    pub fn resolve_close(&self, target: CloseTarget) -> Option<WorkspaceId> {
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
    /// Returns [`OrzmuxError::UnresolvedWorkspace`] for an unknown id;
    /// the order is unchanged.
    pub fn move_to(&mut self, id: WorkspaceId, index: u16) -> OrzmuxResult<bool> {
        let from = self.index_of(id).ok_or(OrzmuxError::UnresolvedWorkspace)?;
        let to = usize::from(index).min(self.order.len().saturating_sub(1));
        if from == to {
            return Ok(false);
        }
        let workspace = self.order.remove(from);
        self.order.insert(to, workspace);
        Ok(true)
    }

    /// Names `id` after `name` with control characters removed, the
    /// surrounding whitespace trimmed, and at most [`MAX_NAME_CHARS`]
    /// characters kept. A name that ends up empty, or `None`, restores the
    /// automatic name. Returns whether the name changed.
    ///
    /// # Errors
    ///
    /// Returns [`OrzmuxError::UnresolvedWorkspace`] for an unknown id.
    pub fn rename(&mut self, id: WorkspaceId, name: Option<String>) -> OrzmuxResult<bool> {
        let workspace = self.get_mut(id).ok_or(OrzmuxError::UnresolvedWorkspace)?;
        let name = name.as_deref().and_then(sanitized_name);
        if workspace.name == name {
            return Ok(false);
        }
        workspace.name = name;
        Ok(true)
    }

    /// Splits `target`, a pane of the displayed workspace, placing `new`
    /// right of / below it.
    ///
    /// # Errors
    ///
    /// Returns [`OrzmuxError::UnresolvedTarget`] when `target` is not in
    /// the displayed workspace, and [`OrzmuxError::SplitRefused`] when the
    /// tree refuses the split.
    pub fn split_active(
        &mut self,
        target: PaneId,
        orientation: SplitOrientation,
        new: PaneId,
        window: GridSize,
    ) -> OrzmuxResult {
        let active = self.active.ok_or(OrzmuxError::UnresolvedTarget)?;
        let workspace = self
            .order
            .iter_mut()
            .find(|w| w.id == active)
            .ok_or(OrzmuxError::UnresolvedTarget)?;
        if !workspace.tree.contains(target) {
            return Err(OrzmuxError::UnresolvedTarget);
        }
        workspace
            .tree
            .split(&mut self.split_ids, target, orientation, new, window)
    }

    /// The displayed workspace's id.
    pub fn active_id(&self) -> Option<WorkspaceId> {
        self.active
    }

    /// The displayed workspace.
    pub fn active(&self) -> Option<&Workspace> {
        self.active.and_then(|id| self.get(id))
    }

    /// The displayed workspace, for a change to its tree.
    pub fn active_mut(&mut self) -> Option<&mut Workspace> {
        let id = self.active?;
        self.get_mut(id)
    }

    /// The workspace `id` names.
    pub fn get(&self, id: WorkspaceId) -> Option<&Workspace> {
        self.order.iter().find(|w| w.id == id)
    }

    /// The workspace `id` names, for a change.
    pub fn get_mut(&mut self, id: WorkspaceId) -> Option<&mut Workspace> {
        self.order.iter_mut().find(|w| w.id == id)
    }

    /// The workspace whose tree holds `pane`.
    pub fn workspace_of(&self, pane: PaneId) -> Option<WorkspaceId> {
        self.order
            .iter()
            .find(|w| w.tree.contains(pane))
            .map(|w| w.id)
    }

    /// Every workspace in display order.
    pub fn iter(&self) -> impl Iterator<Item = &Workspace> {
        self.order.iter()
    }

    /// Every workspace in display order, as the GUI lists them.
    pub fn entries(&self) -> Vec<WorkspaceEntry> {
        self.order
            .iter()
            .map(|w| WorkspaceEntry {
                id: w.id,
                name: w.name.clone(),
            })
            .collect()
    }

    fn index_of(&self, id: WorkspaceId) -> Option<usize> {
        self.order.iter().position(|w| w.id == id)
    }
}

/// `name` without control characters, trimmed, and cut to
/// [`MAX_NAME_CHARS`] characters; `None` when nothing is left.
fn sanitized_name(name: &str) -> Option<String> {
    let kept: String = name.chars().filter(|c| !c.is_control()).collect();
    let cut: String = kept.trim().chars().take(MAX_NAME_CHARS).collect();
    let trimmed = cut.trim_end();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: GridSize = GridSize { cols: 80, rows: 24 };

    fn three() -> (Workspaces, [WorkspaceId; 3]) {
        let mut set = Workspaces::default();
        let a = set.create(PaneId(1));
        let b = set.create(PaneId(2));
        let c = set.create(PaneId(3));
        (set, [a, b, c])
    }

    /// Asserts that only the first workspace becomes displayed on creation
    /// and that ids increase.
    ///
    /// Case: orzma starts with one workspace, then the user opens two more
    /// in the background of the spawn.
    #[test]
    fn the_first_workspace_is_displayed_and_ids_increase() {
        let (set, [a, b, c]) = three();
        assert_eq!(set.active_id(), Some(a));
        assert!(a < b && b < c);
        assert_eq!(set.entries().len(), 3);
    }

    /// Asserts that removing the displayed workspace displays the one that
    /// takes its position, or the new last one when it was last.
    ///
    /// Case: the user closes the displayed tab in the middle, then the
    /// last tab.
    #[test]
    fn removing_the_displayed_workspace_displays_its_right_neighbour() {
        let (mut set, [a, b, c]) = three();
        set.activate(b);
        assert!(set.remove(b));
        assert_eq!(set.active_id(), Some(c));
        assert!(set.remove(c));
        assert_eq!(set.active_id(), Some(a));
        assert!(set.remove(a));
        assert_eq!(set.active_id(), None);
        assert!(set.entries().is_empty());
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
        assert_eq!(set.resolve(WorkspaceTarget::Next), Some(a));
        set.activate(a);
        assert_eq!(set.resolve(WorkspaceTarget::Previous), Some(c));
        assert_eq!(set.resolve(WorkspaceTarget::Index(2)), Some(c));
        assert_eq!(set.resolve(WorkspaceTarget::Index(8)), None);
    }

    /// Asserts that a move clamps to the last position, reports whether the
    /// order changed, and refuses an unknown workspace.
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
            set.move_to(WorkspaceId(99), 0),
            Err(OrzmuxError::UnresolvedWorkspace)
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
        assert_eq!(set.get(a).and_then(|w| w.name.clone()), Some("logs".into()));
        assert_eq!(set.rename(a, Some("logs".into())).ok(), Some(false));
        let long: String = "あ".repeat(70);
        set.rename(a, Some(long)).expect("a known workspace");
        assert_eq!(
            set.get(a)
                .and_then(|w| w.name.clone())
                .map(|n| n.chars().count()),
            Some(MAX_NAME_CHARS)
        );
        assert_eq!(set.rename(a, Some("   ".into())).ok(), Some(true));
        assert_eq!(set.get(a).and_then(|w| w.name.clone()), None);
    }

    /// Asserts that a split must target the displayed workspace and that
    /// split ids stay unique across workspaces.
    ///
    /// Case: the user splits a pane in one workspace, switches, and splits
    /// in the other, while a stale request still names a hidden pane.
    #[test]
    fn splits_target_the_displayed_workspace_with_unique_ids() {
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
                .map(|w| w.tree.solve(W).separators[0].split)
                .expect("a split workspace")
        };
        assert_ne!(split_of(a), split_of(b));
        assert_eq!(set.workspace_of(PaneId(12)), Some(b));
    }
}
