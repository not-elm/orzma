use ozmux_macros::NewType;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{Mutex, MutexGuard};
use crate::{
    SessionId,
    cell::{CellId, LayoutCellState},
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
}
