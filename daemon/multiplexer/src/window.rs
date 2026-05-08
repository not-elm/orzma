use crate::{
    cells::{CellId, CloseOutcome, LayoutCellState, Side, SplitOrientation},
    error::{SessionError, SessionResult},
    pane::{Pane, PaneId},
};
use ozmux_macros::NewType;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{Mutex, MutexGuard};

#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize, NewType)]
#[newtype(as_ref(str), display, new(uuid_v4_string), default)]
pub struct WindowId(String);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Window {
    pub name: String,
    pub root_cell: CellId,
    pub active_pane: PaneId,
}

impl Window {
    /// Construct a window with a single initial pane.
    pub fn new(name: impl Into<String>, root_cell: CellId, active_pane: PaneId) -> Self {
        Self {
            name: name.into(),
            root_cell,
            active_pane,
        }
    }

    pub fn rename(&mut self, name: impl Into<String>) {
        self.name = name.into();
    }

    /// Clone the window for use in HTTP responses (released from any guard).
    pub fn clone_for_response(&self) -> Self {
        self.clone()
    }

    pub fn split_pane(
        &mut self,
        target_pane_id: &PaneId,
        new_pane_id: PaneId,
        orientation: SplitOrientation,
        side: Side,
    ) -> SessionResult<PaneId> {
        let target_cell_id = self.panes.get(target_pane_id)?.cell_id().clone();
        let target_was_root = target_cell_id == self.root;

        let new_cell_id = self.cells.create_pane_cell(new_pane_id.clone(), None);
        self.panes.insert(
            new_pane_id.clone(),
            Pane::new(new_pane_id.clone(), new_cell_id.clone()),
        );

        let new_split_id = self
            .cells
            .split_cell(target_cell_id, new_cell_id, side, orientation)?;

        if target_was_root {
            self.root = new_split_id;
        }

        // Newly created pane becomes active (matches tmux split-window default).
        self.active_pane = new_pane_id.clone();
        Ok(new_pane_id)
    }

    /// Close a pane and propagate the structural change.
    /// Rejects closing the last pane in the window.
    pub fn close_pane(&mut self, pane_id: &PaneId) -> SessionResult {
        let cell_id = self.panes.get(pane_id)?.cell_id().clone();

        if cell_id == self.root && self.panes.len() == 1 {
            return Err(SessionError::CannotCloseLastPaneInWindow(self.id.clone()));
        }

        let outcome = self.cells.close_cell(&cell_id)?;
        let surviving_pane_id = match outcome {
            CloseOutcome::TreeEmptied => {
                return Err(SessionError::CannotCloseLastPaneInWindow(self.id.clone()));
            }
            CloseOutcome::RootReplaced { new_root } => {
                self.root = new_root.clone();
                self.pane_id_for_cell(&new_root)
            }
            CloseOutcome::SiblingPromoted { survivor, .. } => self.pane_id_for_cell(&survivor),
        };

        self.panes.remove(pane_id)?;

        // If the closed pane was active, promote the surviving sibling.
        // If the surviving cell is a split (not a leaf), fall back to any
        // remaining pane in the window — guarantees active_pane never
        // dangles after close.
        if &self.active_pane == pane_id {
            let new_active = surviving_pane_id.or_else(|| self.panes.any_pane_id());
            if let Some(id) = new_active {
                self.active_pane = id;
            }
        }
        Ok(())
    }

    fn pane_id_for_cell(&self, cell_id: &CellId) -> Option<PaneId> {
        self.panes
            .iter()
            .find(|(_, p)| p.cell_id() == cell_id)
            .map(|(id, _)| id.clone())
    }

    pub fn first_pane(&self) -> Option<&Pane> {
        self.panes.iter().next().map(|(_, p)| p)
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct WindowState(HashMap<WindowId, Window>);

impl WindowState {
    #[inline]
    pub fn register(&mut self, id: WindowId, window: Window) {
        self.0.insert(id, window);
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Replace `active_pane` with `new` on every window currently active on
    /// `old`. Used after `split_pane` to mirror tmux's default behavior of
    /// promoting the freshly-created pane to active.
    pub fn replace_active_pane(&mut self, old: &PaneId, new: &PaneId) {
        for window in self.0.values_mut() {
            if &window.active_pane == old {
                window.active_pane = new.clone();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cells::CellId;

    fn window_with_active(active: PaneId) -> Window {
        Window::new("w", CellId::new(), active)
    }

    #[test]
    fn replace_active_pane_swaps_matching_window() {
        let mut state = WindowState::default();
        let target = PaneId::new();
        let other = PaneId::new();
        let new = PaneId::new();
        state.register(WindowId::new(), window_with_active(target.clone()));
        state.register(WindowId::new(), window_with_active(other.clone()));

        state.replace_active_pane(&target, &new);

        let actives: Vec<&PaneId> = state.0.values().map(|w| &w.active_pane).collect();
        assert!(actives.contains(&&new));
        assert!(actives.contains(&&other));
        assert!(!actives.contains(&&target));
    }

    #[test]
    fn replace_active_pane_no_op_when_no_match() {
        let mut state = WindowState::default();
        let active = PaneId::new();
        let unrelated = PaneId::new();
        let new = PaneId::new();
        state.register(WindowId::new(), window_with_active(active.clone()));

        state.replace_active_pane(&unrelated, &new);

        assert_eq!(
            state.0.values().next().unwrap().active_pane,
            active
        );
    }
}

#[derive(Clone, Default)]
pub struct WindowStore(Arc<Mutex<HashMap<WindowId, Window>>>);

impl WindowStore {
    pub async fn lock(&self) -> MutexGuard<'_, HashMap<WindowId, Window>> {
        self.0.lock().await
    }
}
