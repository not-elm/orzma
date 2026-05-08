use ozmux_macros::NewType;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    ops::{Deref, DerefMut},
    sync::Arc,
};
use tokio::sync::{MappedMutexGuard, Mutex, MutexGuard};

pub mod activity;
pub mod cell;
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
    cell::{CellId, LayoutCellState},
    pane::{Pane, PaneId, PaneStore},
    session::{Session, SessionId, SessionState},
    window::WindowState,
};

pub struct MultiplexerService {
    sessions: SessionState,
    windows: WindowState,
    panes: PaneStore,
    cells: LayoutCellState,
    // どのセルが指定のセルを描画しているかを参照するためのマップ
    pane_to_cells: HashMap<PaneId, Vec<CellId>>,
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
        let root_cell = self.cells.new_root();
        let id = WindowId::new();
        let name = format!("Window{}", self.windows.len());
        let activity_id = self.new_activity(Activity::default());
        let pane_id = self.new_pane(root_cell.clone(), activity_id);
        self.windows
            .register(id.clone(), Window::new(name, root_cell, pane_id));
        id
    }

    pub fn new_pane(&mut self, parent_cell: CellId, activity_id: ActivityId) -> PaneId {
        let id = PaneId::new();
        let pane = Pane::new(activity_id);
        self.panes.register(id.clone(), pane);
        let cell_id = self.cells.new_pane(id.clone(), parent_cell);
        self.pane_to_cells
            .entry(id.clone())
            .or_default()
            .push(cell_id);
        id
    }

    pub fn new_activity(&mut self, activity: Activity) -> ActivityId {
        let id = ActivityId::new();
        self.activities.register(id.clone(), activity);
        id
    }
}
