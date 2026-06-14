//! The plain-data projection model and the pure reducer that maintains it.

use crate::enumerate::WindowRow;
use bevy::prelude::Resource;
use tmux_control_parser::{
    Cell, CellDims, ControlEvent, PaneId, SessionId, WindowId, WindowLayout,
};

/// A projected pane: its tmux id and cell geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneModel {
    /// tmux pane id (`%N`).
    pub id: PaneId,
    /// Cell geometry from the window layout.
    pub dims: CellDims,
}

/// Flattens a window layout tree into its panes, in layout order.
///
/// Each `Cell::Leaf` carrying a pane id becomes a [`PaneModel`]; leaves with
/// no id (a layout-grammar artifact) are skipped.
pub fn pane_leaves(layout: &WindowLayout) -> Vec<PaneModel> {
    let mut out = Vec::new();
    collect_leaves(&layout.root, &mut out);
    out
}

fn collect_leaves(cell: &Cell, out: &mut Vec<PaneModel>) {
    match cell {
        Cell::Leaf { dims, pane_id } => {
            if let Some(id) = pane_id {
                out.push(PaneModel {
                    id: PaneId(*id),
                    dims: *dims,
                });
            }
        }
        Cell::Split { children, .. } => {
            for child in children {
                collect_leaves(child, out);
            }
        }
    }
}

/// A projected window: id, active flag, name, and its panes (layout order).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowModel {
    /// tmux window id (`@N`).
    pub id: WindowId,
    /// Whether this is the session's active window.
    pub active: bool,
    /// tmux display index (#{window_index}).
    pub index: u32,
    /// Window name.
    pub name: String,
    /// Panes in layout order.
    pub panes: Vec<PaneModel>,
}

/// The desired projection: the session and its windows, plus the active pane.
///
/// Mutated by the pure reducer ([`ProjectionModel::seed_from_rows`] /
/// [`ProjectionModel::apply_event`]); the ECS reconcile syncs entities to it.
#[derive(Resource, Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectionModel {
    /// The attached session id, once known.
    pub session: Option<SessionId>,
    /// Windows in insertion order.
    pub windows: Vec<WindowModel>,
    /// The currently active pane, once known.
    pub active_pane: Option<PaneId>,
}

impl ProjectionModel {
    /// Replaces the window set from a parsed `list-windows` reply.
    pub fn seed_from_rows(&mut self, rows: &[WindowRow]) {
        self.windows = rows
            .iter()
            .map(|row| WindowModel {
                id: row.id,
                active: row.active,
                index: row.index,
                name: row.name.clone(),
                panes: pane_leaves(&row.layout),
            })
            .collect();
        self.prune_active_pane();
    }

    /// Applies one control-mode notification to the model, returning `true`
    /// if it touched tracked state.
    ///
    /// Events the projection does not track (e.g. `%output`) return `false`
    /// so callers can skip change propagation — without this, a pane
    /// flooding output would mark the model changed every frame and force a
    /// full reconcile pass for no structural change.
    pub fn apply_event(&mut self, event: &ControlEvent) -> bool {
        match event {
            ControlEvent::SessionChanged { session, .. } => {
                self.session = Some(*session);
                true
            }
            ControlEvent::WindowAdd { window } => {
                self.ensure_window(*window);
                true
            }
            ControlEvent::WindowClose { window } => {
                let before = self.windows.len();
                self.windows.retain(|w| w.id != *window);
                let removed = self.windows.len() != before;
                if removed {
                    self.prune_active_pane();
                }
                removed
            }
            ControlEvent::WindowRenamed { window, name } => match self.window_mut(*window) {
                Some(w) => {
                    w.name = name.clone();
                    true
                }
                None => false,
            },
            ControlEvent::LayoutChange { window, layout, .. } => {
                self.set_layout(*window, layout);
                self.prune_active_pane();
                true
            }
            ControlEvent::WindowPaneChanged { window, pane } => {
                self.active_pane = Some(*pane);
                self.ensure_window(*window);
                self.set_active_window(*window);
                true
            }
            _ => false,
        }
    }

    fn ensure_window(&mut self, id: WindowId) -> &mut WindowModel {
        if let Some(idx) = self.windows.iter().position(|w| w.id == id) {
            return &mut self.windows[idx];
        }
        self.windows.push(WindowModel {
            id,
            active: false,
            index: 0,
            name: String::new(),
            panes: Vec::new(),
        });
        self.windows.last_mut().expect("just pushed")
    }

    fn window_mut(&mut self, id: WindowId) -> Option<&mut WindowModel> {
        self.windows.iter_mut().find(|w| w.id == id)
    }

    fn set_layout(&mut self, id: WindowId, layout: &WindowLayout) {
        let panes = pane_leaves(layout);
        self.ensure_window(id).panes = panes;
    }

    fn set_active_window(&mut self, id: WindowId) {
        for w in &mut self.windows {
            w.active = w.id == id;
        }
    }

    fn prune_active_pane(&mut self) {
        if let Some(active) = self.active_pane
            && !self
                .windows
                .iter()
                .any(|w| w.panes.iter().any(|p| p.id == active))
        {
            self.active_pane = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tmux_control_parser::{ControlEvent, PaneId, SessionId, WindowId, WindowLayout};

    fn dims(width: u32, height: u32, xoff: i32, yoff: i32) -> CellDims {
        CellDims {
            width,
            height,
            xoff,
            yoff,
        }
    }

    #[test]
    fn single_pane_layout_yields_one_pane() {
        let layout = WindowLayout::parse(b"b25f,80x24,0,0,0").unwrap();
        assert_eq!(
            pane_leaves(&layout),
            vec![PaneModel {
                id: PaneId(0),
                dims: dims(80, 24, 0, 0),
            }]
        );
    }

    #[test]
    fn horizontal_split_yields_two_panes_in_order() {
        let layout = WindowLayout::parse(b"abcd,80x24,0,0{40x24,0,0,1,39x24,41,0,2}").unwrap();
        let panes = pane_leaves(&layout);
        assert_eq!(panes.len(), 2);
        assert_eq!(panes[0].id, PaneId(1));
        assert_eq!(panes[1].id, PaneId(2));
        assert_eq!(panes[0].dims, dims(40, 24, 0, 0));
        assert_eq!(panes[1].dims, dims(39, 24, 41, 0));
    }

    fn layout(spec: &[u8]) -> WindowLayout {
        WindowLayout::parse(spec).unwrap()
    }

    #[test]
    fn session_changed_sets_session() {
        let mut m = ProjectionModel::default();
        m.apply_event(&ControlEvent::SessionChanged {
            session: SessionId(3),
            name: "main".to_string(),
        });
        assert_eq!(m.session, Some(SessionId(3)));
    }

    #[test]
    fn window_add_then_close() {
        let mut m = ProjectionModel::default();
        m.apply_event(&ControlEvent::WindowAdd {
            window: WindowId(1),
        });
        assert_eq!(m.windows.len(), 1);
        assert_eq!(m.windows[0].id, WindowId(1));
        m.apply_event(&ControlEvent::WindowClose {
            window: WindowId(1),
        });
        assert!(m.windows.is_empty());
    }

    #[test]
    fn layout_change_sets_panes_and_creates_window() {
        let mut m = ProjectionModel::default();
        m.apply_event(&ControlEvent::LayoutChange {
            window: WindowId(7),
            layout: layout(b"abcd,80x24,0,0{40x24,0,0,1,39x24,41,0,2}"),
            visible_layout: layout(b"abcd,80x24,0,0{40x24,0,0,1,39x24,41,0,2}"),
            flags: String::new(),
        });
        assert_eq!(m.windows.len(), 1);
        assert_eq!(m.windows[0].panes.len(), 2);
        assert_eq!(m.windows[0].panes[0].id, PaneId(1));
    }

    #[test]
    fn window_pane_changed_sets_active_pane_and_window() {
        let mut m = ProjectionModel::default();
        m.apply_event(&ControlEvent::WindowAdd {
            window: WindowId(1),
        });
        m.apply_event(&ControlEvent::WindowAdd {
            window: WindowId(2),
        });
        m.apply_event(&ControlEvent::WindowPaneChanged {
            window: WindowId(2),
            pane: PaneId(5),
        });
        assert_eq!(m.active_pane, Some(PaneId(5)));
        assert!(!m.windows[0].active);
        assert!(m.windows[1].active);
    }

    #[test]
    fn seed_from_rows_builds_windows_with_panes() {
        use crate::enumerate::parse_window_rows;
        let rows = parse_window_rows(&[
            "1\t@1\t0\tabcd,80x24,0,0{40x24,0,0,1,39x24,41,0,2}\tx\tmain".to_string(),
        ])
        .unwrap();
        let mut m = ProjectionModel::default();
        m.seed_from_rows(&rows);
        assert_eq!(m.windows.len(), 1);
        assert_eq!(m.windows[0].panes.len(), 2);
        assert!(m.windows[0].active);
    }

    #[test]
    fn window_pane_changed_for_unknown_window_creates_and_activates_it() {
        let mut m = ProjectionModel::default();
        assert!(m.apply_event(&ControlEvent::WindowPaneChanged {
            window: WindowId(9),
            pane: PaneId(3),
        }));
        assert_eq!(m.windows.len(), 1);
        assert_eq!(m.windows[0].id, WindowId(9));
        assert!(m.windows[0].active);
        assert_eq!(m.active_pane, Some(PaneId(3)));
    }

    #[test]
    fn closing_the_window_holding_active_pane_clears_active_pane() {
        let mut m = ProjectionModel::default();
        m.apply_event(&ControlEvent::LayoutChange {
            window: WindowId(1),
            layout: layout(b"abcd,80x24,0,0,7"),
            visible_layout: layout(b"abcd,80x24,0,0,7"),
            flags: String::new(),
        });
        m.apply_event(&ControlEvent::WindowPaneChanged {
            window: WindowId(1),
            pane: PaneId(7),
        });
        assert_eq!(m.active_pane, Some(PaneId(7)));
        m.apply_event(&ControlEvent::WindowClose {
            window: WindowId(1),
        });
        assert!(m.windows.is_empty());
        assert_eq!(m.active_pane, None);
    }

    #[test]
    fn untracked_event_reports_no_change() {
        let mut m = ProjectionModel::default();
        assert!(!m.apply_event(&ControlEvent::Output {
            pane: PaneId(1),
            data: vec![b'x'],
        }));
        assert!(m.windows.is_empty());
    }
}
