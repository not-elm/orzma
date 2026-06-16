//! Global events fired by the drain system and applied by the observers. Each
//! payload carries only tmux-side ids (never an `Entity`); observers resolve
//! ids to entities via the `TmuxProjection` index.

use crate::components::WindowFlags;
use bevy::prelude::Event;
use tmux_control_parser::{Cell, CellDims, Divider, PaneId, SessionId, WindowId, WindowLayout};

/// A pane's tmux id plus its cell geometry, carried in `TmuxLayoutChanged`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PaneGeom {
    /// tmux pane id (`%N`).
    pub(crate) id: PaneId,
    /// Cell geometry from the window layout.
    pub(crate) dims: CellDims,
}

/// `%session-changed`: the attached session and its name.
#[derive(Event, Debug, Clone)]
pub(crate) struct TmuxSessionChanged {
    pub(crate) session: SessionId,
    pub(crate) name: String,
}

/// `%window-add` (defaults) or a seed row (real `index`/`name`).
///
/// A bare `%window-add` notification carries no index or name, so it uses the
/// sentinel `index: 0` / `name: ""`; observers must not treat those as
/// authoritative (the later seed row supplies the real values).
#[derive(Event, Debug, Clone)]
pub(crate) struct TmuxWindowAdded {
    pub(crate) window: WindowId,
    pub(crate) index: u32,
    pub(crate) name: String,
}

/// `%window-close`.
#[derive(Event, Debug, Clone)]
pub(crate) struct TmuxWindowClosed {
    pub(crate) window: WindowId,
}

/// `%window-renamed`.
#[derive(Event, Debug, Clone)]
pub(crate) struct TmuxWindowRenamed {
    pub(crate) window: WindowId,
    pub(crate) name: String,
}

/// A window's `#{window_raw_flags}` changed — from a `%subscription-changed`
/// notification or a seed row.
#[derive(Event, Debug, Clone)]
pub(crate) struct TmuxWindowFlagsChanged {
    pub(crate) window: WindowId,
    pub(crate) flags: WindowFlags,
}

/// `%layout-change` or a seed row: the window's full pane set.
#[derive(Event, Debug, Clone)]
pub(crate) struct TmuxLayoutChanged {
    pub(crate) window: WindowId,
    pub(crate) panes: Vec<PaneGeom>,
    pub(crate) dividers: Vec<Divider>,
}

/// `%window-pane-changed`: the active pane (and its window).
#[derive(Event, Debug, Clone)]
pub(crate) struct TmuxActivePaneChanged {
    pub(crate) window: WindowId,
    pub(crate) pane: PaneId,
}

/// A seed row's active flag: this window is the active one.
#[derive(Event, Debug, Clone)]
pub(crate) struct TmuxActiveWindowChanged {
    pub(crate) window: WindowId,
}

/// Seed prune: despawn every window not in this set.
#[derive(Event, Debug, Clone)]
pub(crate) struct TmuxWindowsRetained {
    pub(crate) windows: Vec<WindowId>,
}

/// Transport `Closed`: tear the whole projection down.
#[derive(Event, Debug, Clone)]
pub(crate) struct TmuxConnectionReset;

/// Flattens a window layout tree into its panes, in layout order. Leaves with
/// no id (a layout-grammar artifact) are skipped.
pub(crate) fn pane_geoms(layout: &WindowLayout) -> Vec<PaneGeom> {
    let mut out = Vec::new();
    collect_leaves(&layout.root, &mut out);
    out
}

fn collect_leaves(cell: &Cell, out: &mut Vec<PaneGeom>) {
    match cell {
        Cell::Leaf { dims, pane_id } => {
            if let Some(id) = pane_id {
                out.push(PaneGeom {
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

#[cfg(test)]
mod tests {
    use super::*;
    use tmux_control_parser::{DividerAxis, dividers};

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
            pane_geoms(&layout),
            vec![PaneGeom {
                id: PaneId(0),
                dims: dims(80, 24, 0, 0)
            }]
        );
    }

    #[test]
    fn horizontal_split_yields_two_panes_in_order() {
        let layout = WindowLayout::parse(b"abcd,80x24,0,0{40x24,0,0,1,39x24,41,0,2}").unwrap();
        let panes = pane_geoms(&layout);
        assert_eq!(panes.len(), 2);
        assert_eq!((panes[0].id, panes[1].id), (PaneId(1), PaneId(2)));
        assert_eq!(panes[0].dims, dims(40, 24, 0, 0));
        assert_eq!(panes[1].dims, dims(39, 24, 41, 0));
    }

    #[test]
    fn left_right_split_yields_one_vertical_divider() {
        let layout = WindowLayout::parse(b"abcd,80x24,0,0{40x24,0,0,1,39x24,41,0,2}").unwrap();
        let ds = dividers(&layout);
        assert_eq!(ds.len(), 1);
        assert_eq!(ds[0].axis, DividerAxis::Vertical);
        assert_eq!(ds[0].primary, PaneId(1));
    }
}
