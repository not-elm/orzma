use crate::layout_dto::{WindowLayoutNode, build_layout};
use ozmux_multiplexer::{
    Activity, ActivityId, ActivityKind, CellId, MultiplexerResult, Pane, PaneId, Window, WindowId,
};
use serde::Serialize;
use std::collections::HashMap;

#[derive(Serialize)]
pub struct WindowView {
    pub id: WindowId,
    pub name: String,
    pub root_cell: CellId,
    pub active_pane: PaneId,
    pub panes: Vec<PaneView>,
    pub layout: WindowLayoutNode,
}

#[derive(Serialize)]
pub struct PaneView {
    pub id: PaneId,
    pub active_activity: ActivityId,
    pub activities: Vec<ActivityView>,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ActivityView {
    Terminal {
        id: ActivityId,
        title: String,
    },
    Extension {
        id: ActivityId,
        title: String,
        iframe_url: String,
    },
}

impl WindowView {
    pub fn from_window(
        window: &Window,
        titles: &HashMap<ActivityId, String>,
    ) -> MultiplexerResult<Self> {
        let pane_ids = window.cells.pane_ids_in_subtree(&window.root_cell)?;
        let panes = pane_ids
            .iter()
            .filter_map(|pid| {
                window
                    .panes
                    .get(pid)
                    .map(|p| PaneView::from_pane(p, &window.id, titles))
            })
            .collect();
        let layout = build_layout(&window.root_cell, &window.cells)?;
        Ok(Self {
            id: window.id.clone(),
            name: window.name.clone(),
            root_cell: window.root_cell.clone(),
            active_pane: window.active_pane.clone(),
            panes,
            layout,
        })
    }
}

impl PaneView {
    fn from_pane(pane: &Pane, wid: &WindowId, titles: &HashMap<ActivityId, String>) -> Self {
        let activities = pane
            .activities
            .iter()
            .map(|a| ActivityView::from_activity(a, wid, &pane.id, titles))
            .collect();
        Self {
            id: pane.id.clone(),
            active_activity: pane.active_activity.clone(),
            activities,
        }
    }
}

impl ActivityView {
    fn from_activity(
        activity: &Activity,
        wid: &WindowId,
        pid: &PaneId,
        titles: &HashMap<ActivityId, String>,
    ) -> Self {
        match &activity.kind {
            ActivityKind::Terminal => Self::Terminal {
                id: activity.id.clone(),
                title: titles
                    .get(&activity.id)
                    .filter(|t| !t.is_empty())
                    .cloned()
                    .unwrap_or_else(|| activity.name.clone()),
            },
            ActivityKind::Extension { .. } => Self::Extension {
                id: activity.id.clone(),
                title: activity.name.clone(),
                iframe_url: format!(
                    "/windows/{}/panes/{}/activities/{}/iframe/index.html",
                    wid, pid, activity.id
                ),
            },
        }
    }
}
