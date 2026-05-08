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
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct WindowState(HashMap<WindowId, Window>);

impl WindowState {
    #[inline]
    pub fn insert(&mut self, id: WindowId, window: Window) {
        self.0.insert(id, window);
    }

    #[inline]
    pub fn get(&self, id: &WindowId) -> Option<&Window> {
        self.0.get(id)
    }

    #[inline]
    pub fn get_mut(&mut self, id: &WindowId) -> Option<&mut Window> {
        self.0.get_mut(id)
    }

    #[inline]
    pub fn remove(&mut self, id: &WindowId) -> Option<Window> {
        self.0.remove(id)
    }

    #[inline]
    pub fn contains_key(&self, id: &WindowId) -> bool {
        self.0.contains_key(id)
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
        state.insert(WindowId::new(), window_with_active(target.clone()));
        state.insert(WindowId::new(), window_with_active(other.clone()));

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
        state.insert(WindowId::new(), window_with_active(active.clone()));

        state.replace_active_pane(&unrelated, &new);

        assert_eq!(state.0.values().next().unwrap().active_pane, active);
    }
}
