use ozmux_multiplexer::{
    Cell, CellId, LayoutCellState, MultiplexerResult, PaneId, SplitOrientation,
};
use serde::Serialize;

#[derive(Debug, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum WindowLayoutNode {
    Root {
        cell_id: CellId,
        child: Box<WindowLayoutNode>,
    },
    Split {
        cell_id: CellId,
        orientation: SplitOrientation,
        split_ratio: f32,
        lhs: Box<WindowLayoutNode>,
        rhs: Box<WindowLayoutNode>,
    },
    Pane {
        cell_id: CellId,
        pane_id: PaneId,
    },
}

pub fn build_layout(
    root_cell_id: &CellId,
    cells: &LayoutCellState,
) -> MultiplexerResult<WindowLayoutNode> {
    build_node(root_cell_id, cells)
}

fn build_node(cell_id: &CellId, cells: &LayoutCellState) -> MultiplexerResult<WindowLayoutNode> {
    match cells.cell(cell_id)? {
        Cell::Root(r) => {
            let child = build_node(&r.child, cells)?;
            Ok(WindowLayoutNode::Root {
                cell_id: cell_id.clone(),
                child: Box::new(child),
            })
        }
        Cell::Split(s) => {
            let split_ratio = LayoutCellState::split_ratio(s.lhs_weight, s.rhs_weight);
            let lhs = build_node(&s.lhs_cell, cells)?;
            let rhs = build_node(&s.rhs_cell, cells)?;
            Ok(WindowLayoutNode::Split {
                cell_id: cell_id.clone(),
                orientation: s.orientation,
                split_ratio,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            })
        }
        Cell::Pane(p) => Ok(WindowLayoutNode::Pane {
            cell_id: cell_id.clone(),
            pane_id: p.pane.clone(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ozmux_multiplexer::Side;

    #[test]
    fn build_layout_for_single_pane_window() {
        let mut cells = LayoutCellState::default();
        let pane = PaneId::new();
        let (root_id, pane_cell_id) = cells.new_window_layout(pane.clone());
        let layout = build_layout(&root_id, &cells).unwrap();
        match layout {
            WindowLayoutNode::Root { cell_id, child } => {
                assert_eq!(cell_id, root_id);
                match *child {
                    WindowLayoutNode::Pane {
                        cell_id: c,
                        pane_id,
                    } => {
                        assert_eq!(c, pane_cell_id);
                        assert_eq!(pane_id, pane);
                    }
                    other => panic!("expected pane child, got {other:?}"),
                }
            }
            other => panic!("expected root, got {other:?}"),
        }
    }

    #[test]
    fn build_layout_after_horizontal_split_gives_split_with_two_panes() {
        let mut cells = LayoutCellState::default();
        let pane_a = PaneId::new();
        let (root_id, pane_a_cell) = cells.new_window_layout(pane_a.clone());
        let pane_b = PaneId::new();
        let pane_b_cell = cells.new_pane(pane_b.clone(), None);
        let target_cell = match cells.cell(&root_id).unwrap() {
            Cell::Root(r) => r.child.clone(),
            _ => unreachable!(),
        };
        cells
            .split_cell(
                target_cell,
                pane_b_cell.clone(),
                Side::After,
                SplitOrientation::Horizontal,
            )
            .unwrap();

        let layout = build_layout(&root_id, &cells).unwrap();
        let WindowLayoutNode::Root { child, .. } = layout else {
            panic!("expected root");
        };
        match *child {
            WindowLayoutNode::Split {
                orientation,
                split_ratio,
                lhs,
                rhs,
                ..
            } => {
                assert_eq!(orientation, SplitOrientation::Horizontal);
                assert!((split_ratio - 0.5).abs() < f32::EPSILON);
                match (*lhs, *rhs) {
                    (
                        WindowLayoutNode::Pane {
                            cell_id: lhs_cell,
                            pane_id: lhs_pane,
                        },
                        WindowLayoutNode::Pane {
                            cell_id: rhs_cell,
                            pane_id: rhs_pane,
                        },
                    ) => {
                        // Side::After puts the new pane on the right.
                        assert_eq!(lhs_cell, pane_a_cell);
                        assert_eq!(lhs_pane, pane_a);
                        assert_eq!(rhs_cell, pane_b_cell);
                        assert_eq!(rhs_pane, pane_b);
                    }
                    other => panic!("expected two pane children, got {other:?}"),
                }
            }
            other => panic!("expected split, got {other:?}"),
        }
    }

    #[test]
    fn compute_split_ratio_handles_zero_sum_with_half() {
        assert_eq!(LayoutCellState::split_ratio(0.0, 0.0), 0.5);
    }

    #[test]
    fn compute_split_ratio_normalizes_unbalanced_weights() {
        let r = LayoutCellState::split_ratio(3.0, 1.0);
        assert!((r - 0.75).abs() < f32::EPSILON);
    }
}
