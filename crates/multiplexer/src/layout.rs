//! Structural types and arithmetic for the entity-based layout tree.
//! Owns `SplitOrientation`, `Side`, `Rect`, `split_ratio`, `normalized_grows`,
//! spawn-bundle helpers (`child_flex`, `split_node_bundle`, `pane_frame_node`),
//! and the read-only `LayoutTree` query view.

use crate::components::{PaneMarker, SplitNode};
use crate::error::{MultiplexerError, MultiplexerResult};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::ui::{FlexDirection, UiRect, Val};

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

/// Normalize a split's two child weights so they are never *both* zero
/// (which would flex-collapse the subtree to zero size). `(0.0, 0.0)` →
/// `(1.0, 1.0)`; any pair with at least one nonzero passes through. This is
/// the single chokepoint for the never-both-zero invariant for split-child
/// `flex_grow` writes.
pub(crate) fn normalized_grows(lhs: f32, rhs: f32) -> (f32, f32) {
    if lhs == 0.0 && rhs == 0.0 {
        (1.0, 1.0)
    } else {
        (lhs, rhs)
    }
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

/// `Node` props that make an entity a flex *child* occupying `grow` share of
/// its parent's main axis with a zero basis (so equal grows split evenly).
/// Merge these onto a Pane/Split node when it becomes a layout child.
pub(crate) fn child_flex(grow: f32) -> Node {
    Node {
        flex_grow: grow,
        flex_basis: Val::Px(0.0),
        width: Val::Percent(100.0),
        height: Val::Percent(100.0),
        ..default()
    }
}

/// The `Node` + `SplitNode` pair for a split container at `orientation`.
/// The caller merges in `child_flex(..)` when this split is itself a child.
pub(crate) fn split_node_bundle(orientation: SplitOrientation) -> (Node, SplitNode) {
    let flex_direction = match orientation {
        SplitOrientation::Horizontal => FlexDirection::Row,
        SplitOrientation::Vertical => FlexDirection::Column,
    };
    (
        Node {
            flex_direction,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
        },
        SplitNode { orientation },
    )
}

/// The `Node` for a Pane frame (flex column; 1px padding for the border the
/// GUI chrome draws in a later plan). Caller merges in `child_flex(..)`.
pub(crate) fn pane_frame_node() -> Node {
    Node {
        flex_direction: FlexDirection::Column,
        width: Val::Percent(100.0),
        height: Val::Percent(100.0),
        padding: UiRect::all(Val::Px(1.0)),
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
/// from `root`. DFS first-child-first order (matches `ordered_panes`). Splits
/// partition their rect by the two children's `flex_grow` ratio.
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

/// Set both children of a split, enforcing the never-both-zero invariant
/// (`set_child_grow` for each; `normalized_grows` maps `(0,0)→(1,1)`).
pub(crate) fn set_split_grows(
    commands: &mut Commands,
    lhs: Entity,
    rhs: Entity,
    lhs_grow: f32,
    rhs_grow: f32,
) {
    let (l, r) = normalized_grows(lhs_grow, rhs_grow);
    set_child_grow(commands, lhs, l);
    set_child_grow(commands, rhs, r);
}

/// Set a layout child's `flex_grow` (with zero basis) without disturbing its
/// other `Node` fields. Used by split / resize / close / swap.
pub(crate) fn set_child_grow(commands: &mut Commands, child: Entity, grow: f32) {
    commands
        .entity(child)
        .entry::<Node>()
        .and_modify(move |mut n| {
            n.flex_grow = grow;
            n.flex_basis = Val::Px(0.0);
        });
}

/// Insert a `Split` into `target`'s current layout slot, reparenting `target`
/// under it alongside `new_pane`. Reuses `target` (and its whole subtree)
/// untouched. `target`'s old slot `flex_grow` transfers to the new split;
/// the two children get equal `1.0` grows. Children are ordered per `side`.
///
/// Reads `target`'s parent + child index + grow via the passed queries, then
/// queues the structural commands. Caller must have already spawned
/// `new_pane` (with its `Node`) but NOT yet parented it.
pub(crate) fn split_in_tree(
    commands: &mut Commands,
    target: Entity,
    new_pane: Entity,
    side: Side,
    orientation: SplitOrientation,
    child_of: &Query<&ChildOf>,
    children: &Query<&Children>,
    nodes: &Query<&Node>,
) {
    let parent = child_of
        .get(target)
        .map(|c| c.parent())
        .expect("target has a parent slot");
    let index = children
        .get(parent)
        .ok()
        .and_then(|kids| kids.iter().position(|e| e == target))
        .unwrap_or(0);
    let target_grow = nodes.get(target).map(|n| n.flex_grow).unwrap_or(1.0);

    let (mut split_node, split_marker) = split_node_bundle(orientation);
    let split_cf = child_flex(target_grow);
    split_node.flex_grow = split_cf.flex_grow;
    split_node.flex_basis = split_cf.flex_basis;
    let split = commands.spawn((split_node, split_marker)).id();

    set_child_grow(commands, target, 1.0);
    set_child_grow(commands, new_pane, 1.0);

    let (first, second) = match side {
        Side::Before => (new_pane, target),
        Side::After => (target, new_pane),
    };
    commands.entity(split).add_children(&[first, second]);

    commands.entity(parent).insert_children(index, &[split]);
}

/// Outcome of `close_in_tree`: the survivor pane that should become active if
/// the closed pane was active.
pub(crate) struct CloseResult {
    /// A representative leaf pane in the promoted survivor subtree.
    pub survivor_pane: Entity,
}

/// Promote `pane`'s sibling into the grandparent slot, then despawn `pane`
/// and its parent split. Returns the leftmost leaf of the survivor subtree
/// (for `ActivePane` repointing). Errors if `pane` is the workspace's only
/// pane (its parent is the layout-root node, not a `Split`).
///
/// Ordering (load-bearing — see spec §Structural operations): the sibling is
/// reparented to the grandparent (at the split's old index, inheriting the
/// split's grow) BEFORE the split is despawned, so the recursive despawn does
/// not take the survivor's subtree.
pub(crate) fn close_in_tree(
    commands: &mut Commands,
    workspace: Entity,
    pane: Entity,
    child_of: &Query<&ChildOf>,
    children: &Query<&Children>,
    nodes: &Query<&Node>,
    splits: &Query<&SplitNode>,
    panes: &Query<(), With<PaneMarker>>,
) -> MultiplexerResult<CloseResult> {
    let split = child_of
        .get(pane)
        .map(|c| c.parent())
        .map_err(|_| MultiplexerError::PaneNotFound(pane))?;
    if splits.get(split).is_err() {
        return Err(MultiplexerError::CannotCloseLastPaneInWorkspace(workspace));
    }
    let kids: Vec<Entity> = children
        .get(split)
        .map(|c| c.iter().collect())
        .unwrap_or_default();
    let sibling = *kids
        .iter()
        .find(|&&e| e != pane)
        .ok_or(MultiplexerError::PaneNotFound(pane))?;
    let grandparent = child_of
        .get(split)
        .map(|c| c.parent())
        .map_err(|_| MultiplexerError::MissingParentCell)?;
    let split_index = children
        .get(grandparent)
        .ok()
        .and_then(|gk| gk.iter().position(|e| e == split))
        .unwrap_or(0);
    let split_grow = nodes.get(split).map(|n| n.flex_grow).unwrap_or(1.0);

    commands
        .entity(grandparent)
        .insert_children(split_index, &[sibling]);
    set_child_grow(commands, sibling, split_grow);

    commands.entity(pane).despawn();
    commands.entity(split).despawn();

    let survivor_pane = leftmost_pane(sibling, children, panes);
    Ok(CloseResult { survivor_pane })
}

/// Walk down lhs-first to the first Pane leaf under `start`.
pub(crate) fn leftmost_pane(
    start: Entity,
    children: &Query<&Children>,
    panes: &Query<(), With<PaneMarker>>,
) -> Entity {
    let mut cur = start;
    loop {
        if panes.get(cur).is_ok() {
            return cur;
        }
        match children.get(cur).ok().and_then(|k| k.iter().next()) {
            Some(first) => cur = first,
            None => return cur,
        }
    }
}

/// Collect every Pane leaf reachable from `root`, in DFS first-child-first
/// order. Mirrors the old `ordered_pane_cells` ordering.
pub(crate) fn ordered_panes(
    root: Entity,
    children: &Query<&Children>,
    panes: &Query<(), With<PaneMarker>>,
) -> Vec<Entity> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if panes.get(node).is_ok() {
            out.push(node);
            continue;
        }
        if let Ok(kids) = children.get(node) {
            for child in kids.iter().rev() {
                stack.push(child);
            }
        }
    }
    out
}

/// Swap two panes' layout positions, swapping their slot `flex_grow` so each
/// SLOT keeps its proportion (slot-pinning). `a` and `b` must both be Pane
/// leaves currently in the tree.
pub(crate) fn swap_in_tree(
    commands: &mut Commands,
    a: Entity,
    b: Entity,
    child_of: &Query<&ChildOf>,
    children: &Query<&Children>,
    nodes: &Query<&Node>,
) {
    if a == b {
        return;
    }
    let pa = child_of.get(a).map(|c| c.parent()).expect("a has parent");
    let pb = child_of.get(b).map(|c| c.parent()).expect("b has parent");
    let ia = children
        .get(pa)
        .ok()
        .and_then(|k| k.iter().position(|e| e == a))
        .unwrap_or(0);
    let ib = children
        .get(pb)
        .ok()
        .and_then(|k| k.iter().position(|e| e == b))
        .unwrap_or(0);
    let ga = nodes.get(a).map(|n| n.flex_grow).unwrap_or(1.0);
    let gb = nodes.get(b).map(|n| n.flex_grow).unwrap_or(1.0);

    commands.entity(pb).insert_children(ib, &[a]);
    commands.entity(pa).insert_children(ia, &[b]);
    set_child_grow(commands, a, gb);
    set_child_grow(commands, b, ga);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_split_grows_clamps_double_zero_to_one_one() {
        assert_eq!(normalized_grows(0.0, 0.0), (1.0, 1.0));
    }

    #[test]
    fn set_split_grows_passes_through_nonzero() {
        assert_eq!(normalized_grows(0.25, 0.75), (0.25, 0.75));
        assert_eq!(normalized_grows(3.0, 0.0), (3.0, 0.0));
    }

    #[test]
    fn split_node_bundle_sets_flex_direction_from_orientation() {
        let (node, split) = split_node_bundle(SplitOrientation::Vertical);
        assert_eq!(node.flex_direction, FlexDirection::Column);
        assert_eq!(split.orientation, SplitOrientation::Vertical);
        let (node_h, _) = split_node_bundle(SplitOrientation::Horizontal);
        assert_eq!(node_h.flex_direction, FlexDirection::Row);
    }

    #[test]
    fn child_flex_uses_zero_basis() {
        let n = child_flex(0.5);
        assert_eq!(n.flex_grow, 0.5);
        assert_eq!(n.flex_basis, Val::Px(0.0));
    }
}
