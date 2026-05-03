use serde::{Deserialize, Serialize};

use crate::session::pane::PaneId;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PaneNode {
    Leaf(PaneId),
    Split {
        orientation: SplitOrientation,
        lhs: SplittedNode,
        rhs: SplittedNode,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SplittedNode {
    pub id: PaneId,
    pub weight: f32,
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub enum SplitOrientation {
    Vertical,
    Horizontal,
}
