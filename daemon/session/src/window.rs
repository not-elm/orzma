use ozmux_macros::NewType;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{Mutex, MutexGuard};
use crate::{
    SessionId,
    cell::{CellId, CloseOutcome, LayoutCellState, Side, SplitOrientation},
    error::{SessionError, SessionResult},
    pane::{Pane, PaneId, PaneStore},
};

#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize, NewType)]
#[newtype(as_ref(str), display, new(uuid_v4_string), default)]
pub struct WindowId(String);

#[derive(Debug, Serialize)]
pub struct Window {
    id: WindowId,
    name: String,
    session_id: SessionId,
    root: CellId,
    cells: LayoutCellState,
    panes: PaneStore,
    active_pane: PaneId,
}

impl Window {
    pub const fn id(&self) -> &WindowId {
        &self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    #[cfg(test)]
    pub fn root(&self) -> &CellId {
        &self.root
    }

    pub fn panes(&self) -> &PaneStore {
        &self.panes
    }

    pub const fn active_pane(&self) -> &PaneId {
        &self.active_pane
    }

    /// Construct a window with a single initial pane.
    pub fn new(id: WindowId, session_id: SessionId, name: String) -> Self {
        let pane_id = PaneId::new();
        let mut cells = LayoutCellState::default();
        let cell_id = cells.create_pane_cell(pane_id.clone(), None);

        let mut panes = PaneStore::default();
        panes.insert(pane_id.clone(), Pane::new(pane_id.clone(), cell_id.clone()));

        Self {
            id,
            name,
            session_id,
            root: cell_id,
            cells,
            panes,
            active_pane: pane_id,
        }
    }

    pub fn rename(&mut self, name: impl Into<String>) {
        self.name = name.into();
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

        let new_split_id =
            self.cells
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
        if &self.active_pane == pane_id {
            if let Some(sibling) = surviving_pane_id {
                self.active_pane = sibling;
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

#[derive(Clone, Default)]
pub struct WindowStore(Arc<Mutex<HashMap<WindowId, Window>>>);

impl WindowStore {
    pub async fn lock(&self) -> MutexGuard<'_, HashMap<WindowId, Window>> {
        self.0.lock().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_window_ids_are_distinct() {
        let a = WindowId::new();
        let b = WindowId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn window_id_displays_as_inner_string() {
        let id = WindowId::new();
        let s: String = id.as_ref().to_string();
        assert!(!s.is_empty());
    }

    #[test]
    fn new_window_has_one_pane_and_active_pane_is_that_pane() {
        let w = Window::new(WindowId::new(), SessionId::new(), "main".into());
        assert_eq!(w.panes().iter().count(), 1);
        let only_pane = w.panes().any_pane_id().unwrap();
        assert_eq!(w.active_pane(), &only_pane);
    }

    #[test]
    fn window_serializes_with_id_name_session_id_root_cells_panes_active_pane() {
        let w = Window::new(WindowId::new(), SessionId::new(), "x".into());
        let v = serde_json::to_value(&w).unwrap();
        assert!(v["id"].is_string());
        assert_eq!(v["name"].as_str(), Some("x"));
        assert!(v["session_id"].is_string());
        assert!(v["root"].is_string());
        assert!(v["cells"].is_object());
        assert!(v["panes"].is_array());
        assert!(v["active_pane"].is_string());
    }

    #[tokio::test]
    async fn window_store_starts_empty() {
        let store = WindowStore::default();
        assert!(store.lock().await.is_empty());
    }

    #[tokio::test]
    async fn window_store_supports_insert_and_get() {
        let store = WindowStore::default();
        let w = Window::new(WindowId::new(), SessionId::new(), String::new());
        let id = w.id().clone();
        store.lock().await.insert(id.clone(), w);
        assert!(store.lock().await.get(&id).is_some());
    }

    use crate::cell::{Side, SplitOrientation};

    #[test]
    fn split_pane_makes_new_pane_active() {
        let mut w = Window::new(WindowId::new(), SessionId::new(), String::new());
        let target = w.panes().any_pane_id().unwrap();
        let new_id = PaneId::new();
        let returned = w
            .split_pane(&target, new_id.clone(), SplitOrientation::Horizontal, Side::After)
            .expect("split should succeed");
        assert_eq!(returned, new_id);
        assert_eq!(w.active_pane(), &new_id);
    }

    #[test]
    fn close_last_pane_returns_cannot_close_last_pane_in_window() {
        let mut w = Window::new(WindowId::new(), SessionId::new(), String::new());
        let only = w.panes().any_pane_id().unwrap();
        let err = w.close_pane(&only).unwrap_err();
        let wid = w.id().clone();
        assert!(matches!(
            err,
            SessionError::CannotCloseLastPaneInWindow(ref id) if id == &wid
        ));
    }

    #[test]
    fn close_pane_promotes_sibling_to_active_when_active_was_closed() {
        let mut w = Window::new(WindowId::new(), SessionId::new(), String::new());
        let original = w.panes().any_pane_id().unwrap();
        let new_id = PaneId::new();
        w.split_pane(&original, new_id.clone(), SplitOrientation::Horizontal, Side::After)
            .expect("split");
        // After split, new_id is active. Close it; original should become active.
        w.close_pane(&new_id).expect("close should succeed");
        assert_eq!(w.active_pane(), &original);
    }
}
