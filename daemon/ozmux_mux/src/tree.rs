//! Layout-tree node types for the arena, the single `set_ratio` invariant
//! chokepoint, and the wire `LayoutNode` serializer (daemon-spec §4-4).

use crate::id::{NodeId, PaneId, SplitId, SurfaceId};
use crate::surface::SurfaceKind;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Split axis.
#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub enum SplitOrientation {
    /// Children share horizontal space (left | right).
    Horizontal,
    /// Children share vertical space (top | bottom).
    Vertical,
}

/// Which side of a target a newly inserted sibling lands on.
#[derive(Clone, Copy, Debug, Default, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub enum Side {
    /// Before the target (left/top).
    Before,
    /// After the target (right/bottom).
    #[default]
    After,
}

/// Internal binary split node. `ratio` is the FIRST child's fraction in
/// `[0, 1]`; it lives on the split slot (so swap is positional). Mutate it
/// ONLY through [`Split::set_ratio`] to keep the invariant single-sourced.
#[derive(Clone, Debug, PartialEq)]
pub struct Split {
    /// Split axis.
    pub orientation: SplitOrientation,
    ratio: f32,
    /// First child (left/top).
    pub first: NodeId,
    /// Second child (right/bottom).
    pub second: NodeId,
    /// Parent node, or `None` at the workspace root.
    pub parent: Option<NodeId>,
}

impl Split {
    /// Constructs a split with the ratio passed through [`Split::set_ratio`].
    pub fn new(
        orientation: SplitOrientation,
        ratio: f32,
        first: NodeId,
        second: NodeId,
        parent: Option<NodeId>,
    ) -> Self {
        let mut s = Split {
            orientation,
            ratio: 0.5,
            first,
            second,
            parent,
        };
        s.set_ratio(ratio);
        s
    }

    /// The first child's fraction, in `[0, 1]`.
    pub fn ratio(&self) -> f32 {
        self.ratio
    }

    /// Sets the ratio, enforcing the invariant: non-finite → `0.5`
    /// (the `(0,0)` rescue analog), otherwise clamp to `[0.0, 1.0]`
    /// inclusive (a fully-collapsed `0.0`/`1.0` is a legal state).
    pub fn set_ratio(&mut self, ratio: f32) {
        self.ratio = if ratio.is_finite() {
            ratio.clamp(0.0, 1.0)
        } else {
            0.5
        };
    }
}

/// Leaf layout node: a pane hosting one or more surfaces.
#[derive(Clone, Debug, PartialEq)]
pub struct Pane {
    /// Surfaces owned by this pane (creation order).
    pub surfaces: Vec<SurfaceId>,
    /// The currently focused surface.
    pub active_surface: SurfaceId,
    /// Parent node, or `None` at the workspace root.
    pub parent: Option<NodeId>,
}

/// Serializable layout subtree for the wire / mirror (daemon-spec §4-4).
/// `Pane` carries resolved `cols`/`rows` (0 when the workspace has no size yet).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum LayoutNode {
    /// Internal split.
    Split {
        /// Stable split id.
        id: SplitId,
        /// Axis.
        orientation: SplitOrientation,
        /// First child's fraction.
        ratio: f32,
        /// First child subtree.
        first: Box<LayoutNode>,
        /// Second child subtree.
        second: Box<LayoutNode>,
    },
    /// Leaf pane with its resolved size and surface kind.
    Pane {
        /// Stable pane id.
        id: PaneId,
        /// The active surface's kind (for the renderer to pick a widget).
        surface_kind: SurfaceKind,
        /// Resolved columns.
        cols: u16,
        /// Resolved rows.
        rows: u16,
    },
}

/// Collects every `NodeId` (splits and panes) referenced by a layout subtree.
pub fn collect_node_ids(node: &LayoutNode, out: &mut HashSet<NodeId>) {
    match node {
        LayoutNode::Split {
            id, first, second, ..
        } => {
            out.insert(NodeId::Split(*id));
            collect_node_ids(first, out);
            collect_node_ids(second, out);
        }
        LayoutNode::Pane { id, .. } => {
            out.insert(NodeId::Pane(*id));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::PaneId;

    #[test]
    fn set_ratio_clamps_and_rescues_non_finite() {
        let mut s = Split {
            orientation: SplitOrientation::Horizontal,
            ratio: 0.5,
            first: NodeId::Pane(PaneId::default()),
            second: NodeId::Pane(PaneId::default()),
            parent: None,
        };
        s.set_ratio(1.5);
        assert_eq!(s.ratio(), 1.0); // clamped inclusive (fully-collapsed legal)
        s.set_ratio(-0.2);
        assert_eq!(s.ratio(), 0.0);
        s.set_ratio(f32::NAN);
        assert_eq!(s.ratio(), 0.5); // non-finite → 0.5 (the (0,0) rescue analog)
        s.set_ratio(0.3);
        assert_eq!(s.ratio(), 0.3);
    }
}
