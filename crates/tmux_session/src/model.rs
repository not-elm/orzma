//! The plain-data projection model and the pure reducer that maintains it.

use tmux_control_parser::{Cell, CellDims, PaneId, WindowLayout};

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
