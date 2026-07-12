//! Bevy-free ratio binary layout tree for multiplexer panes.
//!
//! A window's panes form a binary tree of ratio-based splits. All geometry
//! here is pure and unit-tested; the Bevy layer converts `rects` output into
//! per-pane `Node`s and PTY sizes.

use bevy::ecs::entity::Entity;
use orzma_configs::shortcuts::PaneDirection;

/// The axis a split divides along, named after the divider the user sees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SplitAxis {
    /// Vertical divider: children sit side by side (`first` left, `second` right).
    Vertical,
    /// Horizontal divider: children stack (`first` top, `second` bottom).
    Horizontal,
}

/// A pixel rectangle in window space (origin top-left).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PaneRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// A node in the layout tree: either a pane leaf or a ratio split.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum LayoutNode {
    /// A single pane entity.
    Leaf(Entity),
    /// A split of two subtrees; `ratio` is `first`'s fraction of the axis.
    Split {
        axis: SplitAxis,
        ratio: f32,
        first: Box<LayoutNode>,
        second: Box<LayoutNode>,
    },
}

/// A window's pane layout: the tree plus an optional zoomed pane.
#[derive(Clone, Debug)]
pub(crate) struct MultiplexerLayout {
    root: LayoutNode,
    zoomed: Option<Entity>,
}

impl MultiplexerLayout {
    /// Builds a layout with a single pane leaf.
    pub(crate) fn new(root: Entity) -> Self {
        Self {
            root: LayoutNode::Leaf(root),
            zoomed: None,
        }
    }

    /// Returns every pane leaf in left-to-right / top-to-bottom order.
    pub(crate) fn leaves(&self) -> Vec<Entity> {
        let mut out = Vec::new();
        collect_leaves(&self.root, &mut out);
        out
    }

    /// Whether `pane` is a leaf in this layout.
    pub(crate) fn contains(&self, pane: Entity) -> bool {
        self.leaves().contains(&pane)
    }

    /// Splits `target`'s leaf: `target` becomes `first`, `new_pane` `second`,
    /// ratio 0.5. Returns false (no change) if `target` is not a leaf.
    pub(crate) fn split(&mut self, target: Entity, new_pane: Entity, axis: SplitAxis) -> bool {
        split_at(&mut self.root, target, new_pane, axis)
    }
}

fn collect_leaves(node: &LayoutNode, out: &mut Vec<Entity>) {
    match node {
        LayoutNode::Leaf(e) => out.push(*e),
        LayoutNode::Split { first, second, .. } => {
            collect_leaves(first, out);
            collect_leaves(second, out);
        }
    }
}

fn split_at(node: &mut LayoutNode, target: Entity, new_pane: Entity, axis: SplitAxis) -> bool {
    match node {
        LayoutNode::Leaf(e) if *e == target => {
            *node = LayoutNode::Split {
                axis,
                ratio: 0.5,
                first: Box::new(LayoutNode::Leaf(target)),
                second: Box::new(LayoutNode::Leaf(new_pane)),
            };
            true
        }
        LayoutNode::Leaf(_) => false,
        LayoutNode::Split { first, second, .. } => {
            split_at(first, target, new_pane, axis) || split_at(second, target, new_pane, axis)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(id: u32) -> Entity {
        Entity::from_raw_u32(id).unwrap()
    }

    #[test]
    fn new_single_leaf() {
        let l = MultiplexerLayout::new(e(1));
        assert_eq!(l.leaves(), vec![e(1)]);
        assert!(l.contains(e(1)));
        assert!(!l.contains(e(2)));
    }

    #[test]
    fn split_makes_two_leaves_first_is_target() {
        let mut l = MultiplexerLayout::new(e(1));
        assert!(l.split(e(1), e(2), SplitAxis::Vertical));
        assert_eq!(l.leaves(), vec![e(1), e(2)]);
    }

    #[test]
    fn split_absent_target_is_noop() {
        let mut l = MultiplexerLayout::new(e(1));
        assert!(!l.split(e(9), e(2), SplitAxis::Vertical));
        assert_eq!(l.leaves(), vec![e(1)]);
    }
}
