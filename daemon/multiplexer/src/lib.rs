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
pub mod window_service;

pub use error::{SessionError, SessionResult};
pub use window::{Window, WindowId, WindowStore};
pub use window_service::WindowService;

use crate::{
    activity::{Activity, ActivityId, ActivityState},
    cells::{CellId, LayoutCellState, Side, SplitOrientation},
    pane::{Pane, PaneId, PaneState},
    session::{Session, SessionId, SessionState},
    window::WindowState,
};

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
        let (pane_id, root_cell) = self.new_pane(activity_id, None);
        let name = format!("Window{}", self.windows.len());
        self.windows
            .register(id.clone(), Window::new(name, root_cell, pane_id));
        id
    }

    pub fn new_pane(
        &mut self,
        activity_id: ActivityId,
        parent_cell: Option<CellId>,
    ) -> (PaneId, CellId) {
        let id = PaneId::new();
        self.panes.register(id.clone(), Pane::new(activity_id));
        let cell_id = self.cells.new_pane(id.clone(), parent_cell);
        *self.pane_to_cell.entry(id.clone()).or_default() = cell_id.clone();
        (id, cell_id)
    }

    pub fn new_activity(&mut self, activity: Activity) -> ActivityId {
        let id = ActivityId::new();
        self.activities.register(id.clone(), activity);
        id
    }

    pub fn split_pane(
        &mut self,
        target_pane_id: PaneId,
        side: Side,
        orientation: SplitOrientation,
    ) -> SessionResult<PaneId> {
        let target_cell_id = self.pane_to_cell(&target_pane_id)?.clone();
        let new_activity_id = self.new_activity(Activity::default());
        let (new_pane_id, new_cell_id) = self.new_pane(new_activity_id, None);
        self.cells
            .split_cell(target_cell_id, new_cell_id, side, orientation)?;
        Ok(new_pane_id)
    }

    fn pane_to_cell(&self, pane_id: &PaneId) -> SessionResult<&CellId> {
        self.pane_to_cell
            .get(pane_id)
            .ok_or_else(|| SessionError::CellForPaneNotFound(pane_id.clone()))
    }
}
