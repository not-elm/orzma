use ozmux_macros::NewType;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    ops::{Deref, DerefMut},
    sync::Arc,
};
use tokio::sync::{MappedMutexGuard, Mutex, MutexGuard};

pub mod activity;
pub mod cells;
pub mod error;
pub mod pane;
pub mod session;
pub mod window;

pub use error::{SessionError, SessionResult};
pub use window::*;

use crate::{
    activity::{Activity, ActivityId, ActivityState},
    cells::{CellId, LayoutCellState, Side, SplitOrientation},
    pane::{Pane, PaneId, PaneState},
    session::{Session, SessionId, SessionState},
};

#[derive(Default)]
pub struct MultiplexerService {
    sessions: SessionState,
    windows: WindowState,
    panes: PaneState,
    cells: LayoutCellState,
    // どのセルが指定のセルを描画しているかを参照するためのマップ
    pane_to_cell: HashMap<PaneId, CellId>,
    activities: ActivityState,
}

impl MultiplexerService {
    pub fn new_session(&mut self) {
        let session_id = SessionId::new();
        let window_id = self.new_window();
        let session_name = format!("Session{}", self.sessions.len());
        self.sessions
            .register(session_id, Session::new(session_name, window_id));
    }

    pub fn new_window(&mut self) -> WindowId {
        let id = WindowId::new();
        let activity_id = self.new_activity(Activity::default());
        let pane_id = PaneId::new();
        self.panes.insert(pane_id.clone(), Pane::new(activity_id));
        let (root_cell, pane_cell_id) = self.cells.new_window_layout(pane_id.clone());
        self.pane_to_cell.insert(pane_id.clone(), pane_cell_id);
        let name = format!("Window{}", self.windows.len());
        self.windows
            .insert(id.clone(), Window::new(name, root_cell, pane_id));
        id
    }

    pub fn new_pane(
        &mut self,
        activity_id: ActivityId,
        parent_cell: Option<CellId>,
    ) -> (PaneId, CellId) {
        let id = PaneId::new();
        self.panes.insert(id.clone(), Pane::new(activity_id));
        let cell_id = self.cells.new_pane(id.clone(), parent_cell);
        self.pane_to_cell.insert(id.clone(), cell_id.clone());
        (id, cell_id)
    }

    pub fn new_activity(&mut self, activity: Activity) -> ActivityId {
        let id = ActivityId::new();
        self.activities.insert(id.clone(), activity);
        id
    }

    pub fn split_pane(
        &mut self,
        target_pane_id: PaneId,
        side: Side,
        orientation: SplitOrientation,
    ) -> SessionResult<(PaneId, ActivityId)> {
        let target_cell_id = self.pane_to_cell(&target_pane_id)?.clone();
        let new_activity_id = self.new_activity(Activity::default());
        let (new_pane_id, new_cell_id) = self.new_pane(new_activity_id.clone(), None);
        self.cells
            .split_cell(target_cell_id, new_cell_id, side, orientation)?;
        self.windows
            .replace_active_pane(&target_pane_id, &new_pane_id);
        Ok((new_pane_id, new_activity_id))
    }

    pub fn close_pane(&mut self, pane_id: &PaneId) -> SessionResult {
        let cell_id = self.pane_to_cell(pane_id)?.clone();
        let outcome = self.cells.close_cell(&cell_id)?;
        let survivor_pane_id = self.cells.leftmost_pane(outcome.survivor())?.clone();
        self.windows.replace_active_pane(pane_id, &survivor_pane_id);
        self.forget_pane(pane_id);
        Ok(())
    }

    /// Drop the pane's index entries and its owned activities. Caller is
    /// responsible for already having collapsed the cell tree and rerouted
    /// `active_pane`; this is the final commit step of `close_pane`.
    fn forget_pane(&mut self, pane_id: &PaneId) {
        let pane = self
            .panes
            .remove(pane_id)
            .expect("close_pane validated pane existed before forget_pane");
        self.pane_to_cell.remove(pane_id);
        for activity_id in pane.activities {
            self.activities.remove(&activity_id);
        }
    }

    fn pane_to_cell(&self, pane_id: &PaneId) -> SessionResult<&CellId> {
        self.pane_to_cell
            .get(pane_id)
            .ok_or_else(|| SessionError::CellForPaneNotFound(pane_id.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cells::Cell;

    struct WindowFixture {
        ms: MultiplexerService,
        window_id: WindowId,
        pane_id: PaneId,
        pane_cell: CellId,
        root_cell: CellId,
    }

    fn fresh_window() -> WindowFixture {
        let mut ms = MultiplexerService::default();
        let window_id = ms.new_window();
        let window = ms.windows.get(&window_id).expect("window exists").clone();
        let pane_id = window.active_pane.clone();
        let pane_cell = ms.pane_to_cell(&pane_id).unwrap().clone();
        WindowFixture {
            ms,
            window_id,
            pane_id,
            pane_cell,
            root_cell: window.root_cell,
        }
    }

    #[test]
    fn close_pane_after_split_fully_reverts_state() {
        let WindowFixture {
            mut ms,
            window_id,
            pane_id: original_pane,
            pane_cell: original_cell,
            root_cell,
        } = fresh_window();
        let panes_before = ms.panes.len();

        let (new_pane, new_activity) = ms
            .split_pane(
                original_pane.clone(),
                Side::After,
                SplitOrientation::Horizontal,
            )
            .unwrap();
        assert_eq!(ms.windows.get(&window_id).unwrap().active_pane, new_pane);

        ms.close_pane(&new_pane).unwrap();

        // pane/index/activity for the closed pane are gone.
        assert_eq!(ms.panes.len(), panes_before);
        assert!(!ms.pane_to_cell.contains_key(&new_pane));
        assert!(!ms.activities.contains(&new_activity));

        // active_pane is rerouted back to the surviving original.
        assert_eq!(
            ms.windows.get(&window_id).unwrap().active_pane,
            original_pane
        );

        // The cell tree is collapsed: root.child points at the original pane cell.
        let Cell::Root(root) = ms.cells.cell(&root_cell).unwrap() else {
            panic!("root cell missing");
        };
        assert_eq!(root.child, original_cell);
        let Cell::Pane(pane_cell) = ms.cells.cell(&original_cell).unwrap() else {
            panic!("original pane cell missing");
        };
        assert_eq!(pane_cell.parent.as_ref(), Some(&root_cell));
        assert_eq!(pane_cell.pane, original_pane);
    }

    #[test]
    fn close_last_pane_returns_error_without_mutating_state() {
        let WindowFixture {
            mut ms,
            window_id,
            pane_id,
            pane_cell,
            root_cell,
        } = fresh_window();
        let panes_before = ms.panes.len();

        let result = ms.close_pane(&pane_id);

        assert!(matches!(result, Err(SessionError::CannotCloseLastPane(_))));
        // No store was mutated.
        assert_eq!(ms.panes.len(), panes_before);
        assert_eq!(ms.pane_to_cell(&pane_id).unwrap(), &pane_cell);
        assert!(ms.cells.cell(&pane_cell).is_ok());
        assert!(ms.cells.cell(&root_cell).is_ok());
        assert_eq!(ms.windows.get(&window_id).unwrap().active_pane, pane_id);
    }

    #[test]
    fn close_pane_unknown_id_returns_cell_for_pane_not_found() {
        let mut ms = MultiplexerService::default();
        let unknown = PaneId::new();
        assert!(matches!(
            ms.close_pane(&unknown),
            Err(SessionError::CellForPaneNotFound(_))
        ));
    }

    #[test]
    fn close_non_active_pane_leaves_active_pane_unchanged() {
        let WindowFixture {
            mut ms,
            window_id,
            pane_id: original_pane,
            ..
        } = fresh_window();

        let (new_pane, _) = ms
            .split_pane(
                original_pane.clone(),
                Side::After,
                SplitOrientation::Horizontal,
            )
            .unwrap();
        // Make the original pane active again before closing the new one.
        ms.windows
            .replace_active_pane(&new_pane, &original_pane);

        ms.close_pane(&new_pane).unwrap();

        assert_eq!(
            ms.windows.get(&window_id).unwrap().active_pane,
            original_pane
        );
    }
}
