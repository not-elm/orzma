use crate::{
    SessionState, WindowStore,
    activity::{Activity, ActivityId, ActivityKind, ActivityState},
    cell::LayoutCellState,
    pane::PaneStore,
};

pub struct Service {
    sessions: SessionState,
    windows: WindowStore,
    panes: PaneStore,
    cells: LayoutCellState,
    activities: ActivityState,
}

impl Service {
    pub fn create_activity(&mut self, activity: Activity) -> ActivityId {
        let id = ActivityId::new();
        self.activities.register(id.clone(), activity);
        id
    }
}
