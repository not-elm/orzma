//! Structural types and arithmetic for the entity-based layout tree.
//! Owns `SplitOrientation`, `Side`, `Rect`, `split_ratio`, spawn-bundle
//! helpers (`split_node_bundle`, `pane_frame_node`), and the read-only
//! `LayoutTree` query view.

use crate::components::{PaneMarker, SplitNode};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::ui::{FlexDirection, Val};

/// Split axis.
#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq)]
pub enum SplitOrientation {
    /// Left and right children share horizontal space.
    Horizontal,
    /// Top and bottom children share vertical space.
    Vertical,
}

/// Which side of an existing cell a newly-inserted sibling lands on.
#[derive(Debug, Default, Clone, Copy, Hash, Eq, PartialEq)]
pub enum Side {
    /// Place the new node before the target (left or top).
    Before,
    /// Place the new node after the target (right or bottom).
    #[default]
    After,
}

/// Axis-aligned rectangle in normalized workspace coordinates (`x, y, w, h` ∈ [0, 1]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    /// Left edge in [0, 1].
    pub x: f32,
    /// Top edge in [0, 1].
    pub y: f32,
    /// Width in [0, 1].
    pub w: f32,
    /// Height in [0, 1].
    pub h: f32,
}

/// Normalize a `(lhs_weight, rhs_weight)` pair to a `[0, 1]` ratio (the lhs
/// fraction). Returns `0.5` when both are zero.
pub fn split_ratio(lhs_weight: f32, rhs_weight: f32) -> f32 {
    let total = lhs_weight + rhs_weight;
    if total == 0.0 {
        0.5
    } else {
        lhs_weight / total
    }
}

/// The `Node` + `SplitNode` pair for a split container at `orientation`.
///
/// Both axes are `Val::Auto` so taffy fits the container to the sum of its
/// fixed-px children (pane leaves sized by `size_pane_leaves` each frame).
/// The top-level split is the only child of the 100 %/100 % layout-root
/// container, which anchors the whole tile tree to the viewport. Nested
/// splits compose correctly because each child's px size is Mux-determined
/// and the children always sum to the parent's slot.
pub(crate) fn split_node_bundle(orientation: SplitOrientation) -> (Node, SplitNode) {
    let flex_direction = match orientation {
        SplitOrientation::Horizontal => FlexDirection::Row,
        SplitOrientation::Vertical => FlexDirection::Column,
    };
    // NOTE: Val::Auto lets taffy derive both axes from the fixed-px children.
    // A Horizontal split's width = sum(children widths), height = children
    // height; a Vertical split's height = sum(children heights), width =
    // children width.  This is correct because the Mux guarantees children
    // cells sum exactly to the parent slot.
    (
        Node {
            flex_direction,
            width: Val::Auto,
            height: Val::Auto,
            ..default()
        },
        SplitNode { orientation },
    )
}

/// The `Node` for a Pane frame (flex column; width/height are overwritten each
/// frame by the `size_pane_leaves` render system to the Mux-resolved cell rect
/// in logical px).
pub(crate) fn pane_frame_node() -> Node {
    Node {
        flex_direction: FlexDirection::Column,
        width: Val::Percent(100.0),
        height: Val::Percent(100.0),
        ..default()
    }
}

/// Read-only view of the layout entity tree, used by resize / navigation /
/// traversal so the math doesn't thread several raw queries everywhere.
#[derive(SystemParam)]
pub struct LayoutTree<'w, 's> {
    splits: Query<'w, 's, &'static SplitNode>,
    panes: Query<'w, 's, (), With<PaneMarker>>,
    nodes: Query<'w, 's, &'static Node>,
    child_of: Query<'w, 's, &'static ChildOf>,
    children: Query<'w, 's, &'static Children>,
}

impl LayoutTree<'_, '_> {
    /// `true` if `e` is a Pane leaf.
    pub fn is_pane(&self, e: Entity) -> bool {
        self.panes.get(e).is_ok()
    }

    /// The split's orientation, or `None` if `e` is not a `Split`.
    pub fn orientation(&self, e: Entity) -> Option<SplitOrientation> {
        self.splits.get(e).ok().map(|s| s.orientation)
    }

    /// The parent layout node, or `None` at the layout-root node.
    pub fn parent(&self, e: Entity) -> Option<Entity> {
        self.child_of.get(e).ok().map(|c| c.parent())
    }

    /// The `flex_grow` weight of a layout child (0.0 if unset).
    pub fn grow(&self, e: Entity) -> f32 {
        self.nodes.get(e).map(|n| n.flex_grow).unwrap_or(0.0)
    }

    /// The two children of a `Split`, in `(lhs, rhs)` order, or `None` if `e`
    /// is not a split with exactly two children.
    pub fn split_children(&self, e: Entity) -> Option<(Entity, Entity)> {
        let kids = self.children.get(e).ok()?;
        let mut it = kids.iter();
        let a = it.next()?;
        let b = it.next()?;
        it.next().is_none().then_some((a, b))
    }

    /// The single child of a node (the layout-root has exactly one), or `None`.
    fn only_child(&self, e: Entity) -> Option<Entity> {
        self.children.get(e).ok().and_then(|k| {
            let mut it = k.iter();
            let first = it.next()?;
            if it.next().is_none() {
                Some(first)
            } else {
                None
            }
        })
    }
}

/// Compute each Pane leaf's normalized rectangle by walking the entity tree
/// from `root`. DFS first-child-first order. Splits partition their rect by
/// the two children's `flex_grow` ratio.
pub(crate) fn pane_bounds(tree: &LayoutTree, root: Entity) -> Vec<(Entity, Rect)> {
    let mut out = Vec::new();
    walk_bounds(
        tree,
        root,
        Rect {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        },
        &mut out,
    );
    out
}

fn walk_bounds(tree: &LayoutTree, node: Entity, bounds: Rect, out: &mut Vec<(Entity, Rect)>) {
    if tree.is_pane(node) {
        out.push((node, bounds));
        return;
    }
    let Some((lhs, rhs)) = tree.split_children(node) else {
        if let Some(only) = tree.only_child(node) {
            walk_bounds(tree, only, bounds, out);
        }
        return;
    };
    let ratio = split_ratio(tree.grow(lhs), tree.grow(rhs));
    match tree.orientation(node) {
        Some(SplitOrientation::Horizontal) => {
            let lw = bounds.w * ratio;
            walk_bounds(tree, lhs, Rect { w: lw, ..bounds }, out);
            walk_bounds(
                tree,
                rhs,
                Rect {
                    x: bounds.x + lw,
                    w: bounds.w - lw,
                    ..bounds
                },
                out,
            );
        }
        Some(SplitOrientation::Vertical) => {
            let lh = bounds.h * ratio;
            walk_bounds(tree, lhs, Rect { h: lh, ..bounds }, out);
            walk_bounds(
                tree,
                rhs,
                Rect {
                    y: bounds.y + lh,
                    h: bounds.h - lh,
                    ..bounds
                },
                out,
            );
        }
        // NOTE: a node with two children but no orientation is not a valid
        // `SplitNode`; reaching here means the tree invariant is broken.
        None => debug_assert!(
            false,
            "split node {node:?} has two children but no SplitNode orientation"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_node_bundle_sets_flex_direction_from_orientation() {
        let (node, split) = split_node_bundle(SplitOrientation::Vertical);
        assert_eq!(node.flex_direction, FlexDirection::Column);
        assert_eq!(split.orientation, SplitOrientation::Vertical);
        let (node_h, _) = split_node_bundle(SplitOrientation::Horizontal);
        assert_eq!(node_h.flex_direction, FlexDirection::Row);
    }

    #[test]
    fn split_node_bundle_uses_auto_sizing() {
        let (node, _) = split_node_bundle(SplitOrientation::Horizontal);
        assert_eq!(node.width, Val::Auto, "split container width must be Auto");
        assert_eq!(
            node.height,
            Val::Auto,
            "split container height must be Auto"
        );
    }
}
