use std::{
    collections::HashMap,
    ops::{Deref, DerefMut},
    sync::Arc,
};

use serde::Serialize;
use tokio::sync::{MappedMutexGuard, Mutex, MutexGuard};

use crate::{
    define_string_new_type,
    error::{OzmuxError, OzmuxResult},
    session::{
        cell::{CellId, CloseOutcome, LayoutCellState, Side, SplitOrientation},
        pane::{Pane, PaneId, PaneStore},
    },
};

mod activity;
pub mod cell;
pub mod pane;

#[derive(Clone, Default)]
pub struct SessionState(Arc<Mutex<HashMap<SessionId, Session>>>);

/// Read-only guard returned by [`SessionState::session`].
///
/// Holds the underlying mutex lock until dropped; only `Deref` is exposed
/// so callers cannot mutate the session through this type.
#[derive(Debug)]
pub struct SessionRef<'a>(MappedMutexGuard<'a, Session>);

impl Deref for SessionRef<'_> {
    type Target = Session;

    fn deref(&self) -> &Session {
        &self.0
    }
}

/// Exclusive guard returned by [`SessionState::session_mut`].
///
/// Holds the underlying mutex lock until dropped. Implements both `Deref`
/// and `DerefMut`, so callers can mutate the session in place and serialize
/// the result while still holding the lock.
#[derive(Debug)]
pub struct SessionGuard<'a>(MappedMutexGuard<'a, Session>);

impl Deref for SessionGuard<'_> {
    type Target = Session;

    fn deref(&self) -> &Session {
        &self.0
    }
}

impl DerefMut for SessionGuard<'_> {
    fn deref_mut(&mut self) -> &mut Session {
        &mut self.0
    }
}

impl SessionState {
    pub async fn lock(&self) -> MutexGuard<'_, HashMap<SessionId, Session>> {
        self.0.lock().await
    }

    pub async fn session(&self, id: &SessionId) -> OzmuxResult<SessionRef<'_>> {
        let guard = self.0.lock().await;
        let session = MutexGuard::try_map(guard, |sessions| sessions.get_mut(id))
            .map_err(|_| OzmuxError::SessionNotFound(id.clone()))?;
        Ok(SessionRef(session))
    }

    pub async fn session_mut(&self, id: &SessionId) -> OzmuxResult<SessionGuard<'_>> {
        let guard = self.0.lock().await;
        let session = MutexGuard::try_map(guard, |sessions| sessions.get_mut(id))
            .map_err(|_| OzmuxError::SessionNotFound(id.clone()))?;
        Ok(SessionGuard(session))
    }

    pub async fn remove(&self, id: &SessionId) -> OzmuxResult<Session> {
        let mut guard = self.0.lock().await;
        guard
            .remove(id)
            .ok_or_else(|| OzmuxError::SessionNotFound(id.clone()))
    }
}

define_string_new_type!(SessionId);

#[derive(Debug, Serialize)]
pub struct Session {
    id: SessionId,
    name: String,
    root: CellId,
    cells: LayoutCellState,
    panes: PaneStore,
}

impl Session {
    pub fn new(name: String) -> Self {
        let id = SessionId::new();
        let pane_id = PaneId::new();
        let mut cells = LayoutCellState::default();
        let cell_id = cells.create_pane_cell(pane_id.clone(), None);

        let mut panes = PaneStore::default();
        panes.insert(pane_id.clone(), Pane::new(pane_id, cell_id.clone()));

        Self {
            id,
            name,
            root: cell_id,
            cells,
            panes,
        }
    }

    pub const fn id(&self) -> &SessionId {
        &self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn rename(&mut self, name: impl Into<String>) {
        self.name = name.into();
    }

    #[cfg(test)]
    pub fn root(&self) -> &CellId {
        &self.root
    }

    pub fn split_pane(
        &mut self,
        pane_id: &PaneId,
        orientation: SplitOrientation,
        side: Side,
    ) -> OzmuxResult<PaneId> {
        let target_cell_id = self.panes.get(pane_id)?.cell_id().clone();
        let target_was_root = target_cell_id == self.root;

        let new_pane_id = PaneId::new();
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

        Ok(new_pane_id)
    }

    /// Close a pane and propagate the structural change.
    ///
    /// Rejects closing the last pane (returns `CannotCloseLastPane`); closing the only
    /// pane equals ending the session, which is a separate API.
    ///
    /// On success: the pane is removed from `PaneStore`, the layout cell tree is
    /// updated via sibling-promote, and `self.root` is updated if the root changed.
    pub fn close_pane(&mut self, pane_id: &PaneId) -> OzmuxResult {
        let cell_id = self.panes.get(pane_id)?.cell_id().clone();

        // Pre-check: closing the only cell empties the tree, which equals "session ended".
        // The session-level invariant is "≥1 pane"; reject before mutating.
        if cell_id == self.root && self.panes.len() == 1 {
            return Err(OzmuxError::CannotCloseLastPane(pane_id.clone()));
        }

        let outcome = self.cells.close_cell(&cell_id)?;
        match outcome {
            CloseOutcome::TreeEmptied => {
                // Defensive: should be unreachable given the pre-check above.
                return Err(OzmuxError::CannotCloseLastPane(pane_id.clone()));
            }
            CloseOutcome::RootReplaced { new_root } => {
                self.root = new_root;
            }
            CloseOutcome::SiblingPromoted { .. } => {}
        }

        self.panes.remove(pane_id)?;
        Ok(())
    }
}

#[cfg(test)]
impl Session {
    pub(crate) fn panes(&self) -> &PaneStore {
        &self.panes
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new(String::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::OzmuxError;

    #[test]
    fn default_session_root_points_to_initial_pane() {
        let store = Session::default();
        let root_id = store.root();
        let pane_id = store.panes().any_pane_id().expect("default has 1 pane");
        let pane = store.panes().get(&pane_id).expect("pane should exist");
        assert_eq!(pane.cell_id(), root_id);
    }

    #[test]
    fn close_pane_on_last_pane_returns_err() {
        let mut store = Session::default();
        let last_pane_id = store.panes().any_pane_id().expect("default has 1 pane");

        let result = store.close_pane(&last_pane_id);
        assert!(matches!(
            result,
            Err(OzmuxError::CannotCloseLastPane(ref id)) if id == &last_pane_id
        ));
    }

    #[test]
    fn close_pane_after_split_removes_one_pane_and_updates_root() {
        let mut store = Session::default();
        let first_pane_id = store.panes().any_pane_id().expect("default has 1 pane");
        let original_root = store.root().clone();

        store
            .split_pane(&first_pane_id, SplitOrientation::Horizontal, Side::After)
            .expect("split");
        let root_after_split = store.root().clone();
        assert_ne!(
            root_after_split, original_root,
            "split of root must change root id"
        );

        store
            .close_pane(&first_pane_id)
            .expect("close_pane should succeed");

        assert!(
            store.panes().get(&first_pane_id).is_err(),
            "first_pane_id should be removed from PaneStore"
        );
        assert_ne!(
            store.root(),
            &root_after_split,
            "root should have changed (the root split is now collapsed)"
        );
    }

    #[test]
    fn close_pane_under_nested_split_keeps_root_unchanged() {
        // Build a 3-pane layout: split twice. Closing one of the leaves under the
        // inner split should NOT change the root.
        let mut store = Session::default();
        let first_pane_id = store.panes().any_pane_id().unwrap();
        store
            .split_pane(&first_pane_id, SplitOrientation::Horizontal, Side::After)
            .expect("first split");
        let root_after_first = store.root().clone();

        // The new pane created by the split — find its id (the one that's not first_pane_id).
        let pane_ids: Vec<PaneId> = store.panes().iter().map(|(id, _)| id.clone()).collect();
        let second_pane_id = pane_ids
            .into_iter()
            .find(|id| id != &first_pane_id)
            .expect("second pane should exist");

        // Split second_pane_id again to create a 3-pane layout.
        store
            .split_pane(&second_pane_id, SplitOrientation::Vertical, Side::After)
            .expect("second split");
        let root_after_second = store.root().clone();
        assert_eq!(
            root_after_first, root_after_second,
            "splitting a non-root pane must NOT change root"
        );

        // Close one of the inner leaves (second_pane_id).
        store.close_pane(&second_pane_id).expect("close");

        // Root unchanged because the closed pane's parent was not the root split.
        assert_eq!(store.root(), &root_after_second);
        assert!(store.panes().get(&second_pane_id).is_err());
    }

    #[test]
    fn close_pane_with_nonexistent_pane_id_returns_err() {
        let mut store = Session::default();
        let nonexistent = PaneId::new();
        let result = store.close_pane(&nonexistent);
        assert!(matches!(result, Err(OzmuxError::PaneNotFound(_))));
    }

    #[test]
    fn session_carries_its_id() {
        let s = Session::new("demo".to_string());
        assert!(!s.id().as_ref().is_empty());
        assert_eq!(s.name(), "demo");
    }

    #[test]
    fn two_new_sessions_get_distinct_ids() {
        let a = Session::new(String::new());
        let b = Session::new(String::new());
        assert_ne!(a.id(), b.id());
    }

    #[test]
    fn session_serializes_with_id_name_root_cells_panes() {
        let s = Session::new("hello".to_string());
        let v = serde_json::to_value(&s).unwrap();

        assert_eq!(v["id"].as_str(), Some(s.id().as_ref()));
        assert_eq!(v["name"].as_str(), Some("hello"));
        assert!(v["root"].is_string(), "root must be a CellId string");
        assert!(v["cells"].is_object(), "cells must be a flat map");
        assert!(v["panes"].is_array(), "panes must be an array");
        assert_eq!(v["panes"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn session_with_horizontal_split_serializes_cells_with_lowercase_orientation() {
        let mut s = Session::new(String::new());
        let pane_id = s.panes().any_pane_id().unwrap();
        s.split_pane(&pane_id, SplitOrientation::Horizontal, Side::After)
            .expect("split");
        let v = serde_json::to_value(&s).unwrap();
        let cells = v["cells"].as_object().unwrap();
        let split_present = cells.values().any(|c| {
            c["cell"]
                .get("Split")
                .and_then(|s| s.get("orientation"))
                .and_then(|o| o.as_str())
                == Some("horizontal")
        });
        assert!(
            split_present,
            "expected a Split cell with lowercase orientation"
        );
    }

    #[test]
    fn split_pane_returns_new_pane_id() {
        let mut s = Session::new(String::new());
        let pane_id = s.panes().any_pane_id().unwrap();

        let new_id = s
            .split_pane(&pane_id, SplitOrientation::Horizontal, Side::After)
            .expect("split");

        assert!(s.panes().get(&new_id).is_ok());
        assert_ne!(new_id, pane_id);
    }

    #[tokio::test]
    async fn session_state_lock_starts_empty() {
        let state = SessionState::default();
        let guard = state.lock().await;
        assert!(guard.is_empty());
    }

    #[tokio::test]
    async fn session_state_lock_allows_insert_then_get() {
        let state = SessionState::default();
        let session = Session::new("a".to_string());
        let id = session.id().clone();
        {
            let mut guard = state.lock().await;
            guard.insert(id.clone(), session);
        }
        let guard = state.lock().await;
        assert_eq!(guard.get(&id).map(|s| s.name()), Some("a"));
    }

    #[tokio::test]
    async fn session_returns_ref_for_existing_id() {
        let state = SessionState::default();
        let session = Session::new("hello".to_string());
        let id = session.id().clone();
        state.lock().await.insert(id.clone(), session);

        let session_ref = state.session(&id).await.expect("session exists");
        assert_eq!(session_ref.name(), "hello");
    }

    #[tokio::test]
    async fn session_returns_err_for_unknown_id() {
        let state = SessionState::default();
        let id = SessionId::new();
        let err = state.session(&id).await.unwrap_err();
        assert!(matches!(err, OzmuxError::SessionNotFound(ref got) if got == &id));
    }

    #[tokio::test]
    async fn session_mut_allows_in_place_mutation() {
        let state = SessionState::default();
        let session = Session::new("old".to_string());
        let id = session.id().clone();
        state.lock().await.insert(id.clone(), session);

        {
            let mut guard = state.session_mut(&id).await.expect("session exists");
            guard.rename("new");
        }
        assert_eq!(state.session(&id).await.unwrap().name(), "new");
    }

    #[tokio::test]
    async fn session_mut_returns_err_for_unknown_id() {
        let state = SessionState::default();
        let id = SessionId::new();
        let err = state.session_mut(&id).await.unwrap_err();
        assert!(matches!(err, OzmuxError::SessionNotFound(ref got) if got == &id));
    }

    #[tokio::test]
    async fn remove_returns_session_and_removes_it() {
        let state = SessionState::default();
        let session = Session::new(String::new());
        let id = session.id().clone();
        state.lock().await.insert(id.clone(), session);

        let removed = state.remove(&id).await.expect("session exists");
        assert_eq!(removed.id(), &id);
        assert!(state.lock().await.get(&id).is_none());
    }

    #[tokio::test]
    async fn remove_returns_err_for_unknown_id() {
        let state = SessionState::default();
        let id = SessionId::new();
        let err = state.remove(&id).await.unwrap_err();
        assert!(matches!(err, OzmuxError::SessionNotFound(ref got) if got == &id));
    }
}
