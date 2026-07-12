//! Bevy-free ratio binary layout tree for multiplexer panes.
//!
//! A window's panes form a binary tree of ratio-based splits. All geometry
//! here is pure and unit-tested; the Bevy layer converts `rects` output into
//! per-pane `Node`s and PTY sizes.

use bevy::ecs::entity::Entity;
use orzma_configs::shortcuts::PaneDirection;
use std::mem::replace;

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
        collect_leaves(&mut out, &self.root);
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

    /// Computes each pane's pixel rect within `area`, leaving `gap` px between
    /// siblings. When a pane is zoomed it is the only rect returned, filling
    /// `area`.
    pub(crate) fn rects(&self, area: PaneRect, gap: f32) -> Vec<(Entity, PaneRect)> {
        if let Some(z) = self.zoomed
            && self.contains(z)
        {
            return vec![(z, area)];
        }
        let mut out = Vec::new();
        layout_rects(&mut out, &self.root, area, gap);
        out
    }

    /// Sets or clears the zoomed pane.
    pub(crate) fn set_zoom(&mut self, pane: Option<Entity>) {
        self.zoomed = pane;
    }

    /// The currently zoomed pane, if any.
    pub(crate) fn zoomed(&self) -> Option<Entity> {
        self.zoomed
    }

    /// Removes `pane`'s leaf, collapsing its parent split into the surviving
    /// sibling. Returns a leaf of that sibling to focus next, or `None` if
    /// `pane` was the last leaf (caller closes the window). Clears zoom if the
    /// zoomed pane was removed.
    pub(crate) fn remove(&mut self, pane: Entity) -> Option<Entity> {
        if self.zoomed == Some(pane) {
            self.zoomed = None;
        }
        match &self.root {
            LayoutNode::Leaf(e) if *e == pane => None,
            LayoutNode::Leaf(_) => None,
            _ => remove_in(&mut self.root, pane),
        }
    }

    /// The adjacent pane in `dir` from `from`, by edge adjacency with
    /// perpendicular-span overlap. Returns the first overlapping neighbor on
    /// that side, or `None` at the window edge.
    pub(crate) fn neighbor(
        &self,
        from: Entity,
        dir: PaneDirection,
        area: PaneRect,
        gap: f32,
    ) -> Option<Entity> {
        let rects = self.rects(area, gap);
        let src = rects.iter().find(|(e, _)| *e == from)?.1;
        rects
            .iter()
            .filter(|(e, _)| *e != from)
            .filter(|(_, r)| adjacent(src, *r, dir) && perp_overlap(src, *r, dir))
            .map(|(e, _)| *e)
            .next()
    }

    /// Grows or shrinks `focused`'s pane along `dir` by `delta_frac`, moving
    /// the ratio of the nearest ancestor split whose axis matches `dir`
    /// (skipping ancestors on the other axis), clamping each side to
    /// `min_frac`. Returns false (no change) when `focused` has no ancestor
    /// split along that axis (e.g. a lone pane).
    pub(crate) fn resize(
        &mut self,
        focused: Entity,
        dir: PaneDirection,
        delta_frac: f32,
        min_frac: f32,
    ) -> bool {
        let want = axis_of(dir);
        resize_in(&mut self.root, focused, dir, want, delta_frac, min_frac).is_some()
    }
}

fn collect_leaves(out: &mut Vec<Entity>, node: &LayoutNode) {
    match node {
        LayoutNode::Leaf(e) => out.push(*e),
        LayoutNode::Split { first, second, .. } => {
            collect_leaves(out, first);
            collect_leaves(out, second);
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

fn layout_rects(out: &mut Vec<(Entity, PaneRect)>, node: &LayoutNode, area: PaneRect, gap: f32) {
    match node {
        LayoutNode::Leaf(e) => out.push((*e, area)),
        LayoutNode::Split {
            axis,
            ratio,
            first,
            second,
        } => {
            let (a, b) = split_rect(area, *axis, *ratio, gap);
            layout_rects(out, first, a, gap);
            layout_rects(out, second, b, gap);
        }
    }
}

fn split_rect(area: PaneRect, axis: SplitAxis, ratio: f32, gap: f32) -> (PaneRect, PaneRect) {
    match axis {
        SplitAxis::Vertical => {
            let usable = (area.w - gap).max(0.0);
            let fw = (usable * ratio).round();
            let sw = usable - fw;
            (
                PaneRect {
                    x: area.x,
                    y: area.y,
                    w: fw,
                    h: area.h,
                },
                PaneRect {
                    x: area.x + fw + gap,
                    y: area.y,
                    w: sw,
                    h: area.h,
                },
            )
        }
        SplitAxis::Horizontal => {
            let usable = (area.h - gap).max(0.0);
            let fh = (usable * ratio).round();
            let sh = usable - fh;
            (
                PaneRect {
                    x: area.x,
                    y: area.y,
                    w: area.w,
                    h: fh,
                },
                PaneRect {
                    x: area.x,
                    y: area.y + fh + gap,
                    w: area.w,
                    h: sh,
                },
            )
        }
    }
}

fn remove_in(node: &mut LayoutNode, pane: Entity) -> Option<Entity> {
    let LayoutNode::Split { first, second, .. } = node else {
        return None;
    };
    if matches!(first.as_ref(), LayoutNode::Leaf(e) if *e == pane) {
        let sib = replace(second.as_mut(), LayoutNode::Leaf(pane));
        let focus = first_leaf(&sib);
        *node = sib;
        return Some(focus);
    }
    if matches!(second.as_ref(), LayoutNode::Leaf(e) if *e == pane) {
        let sib = replace(first.as_mut(), LayoutNode::Leaf(pane));
        let focus = first_leaf(&sib);
        *node = sib;
        return Some(focus);
    }
    remove_in(first, pane).or_else(|| remove_in(second, pane))
}

fn first_leaf(node: &LayoutNode) -> Entity {
    match node {
        LayoutNode::Leaf(e) => *e,
        LayoutNode::Split { first, .. } => first_leaf(first),
    }
}

fn axis_of(dir: PaneDirection) -> SplitAxis {
    match dir {
        PaneDirection::Left | PaneDirection::Right => SplitAxis::Vertical,
        PaneDirection::Up | PaneDirection::Down => SplitAxis::Horizontal,
    }
}

fn adjacent(src: PaneRect, other: PaneRect, dir: PaneDirection) -> bool {
    let eps = 2.0;
    match dir {
        PaneDirection::Right => {
            (other.x - (src.x + src.w)).abs() <= src.w + other.w && other.x >= src.x + src.w - eps
        }
        PaneDirection::Left => other.x + other.w <= src.x + eps,
        PaneDirection::Down => other.y >= src.y + src.h - eps,
        PaneDirection::Up => other.y + other.h <= src.y + eps,
    }
}

fn perp_overlap(src: PaneRect, other: PaneRect, dir: PaneDirection) -> bool {
    match dir {
        PaneDirection::Left | PaneDirection::Right => {
            src.y < other.y + other.h && other.y < src.y + src.h
        }
        PaneDirection::Up | PaneDirection::Down => {
            src.x < other.x + other.w && other.x < src.x + src.w
        }
    }
}

fn resize_in(
    node: &mut LayoutNode,
    focused: Entity,
    dir: PaneDirection,
    want: SplitAxis,
    delta: f32,
    min: f32,
) -> Option<()> {
    let LayoutNode::Split {
        axis,
        ratio,
        first,
        second,
    } = node
    else {
        return None;
    };
    let in_first = subtree_contains(first, focused);
    let in_second = subtree_contains(second, focused);
    let deeper = if in_first {
        resize_in(first, focused, dir, want, delta, min)
    } else if in_second {
        resize_in(second, focused, dir, want, delta, min)
    } else {
        None
    };
    if deeper.is_some() {
        return deeper;
    }
    if *axis != want || (!in_first && !in_second) {
        return None;
    }
    let toward_second = matches!(dir, PaneDirection::Right | PaneDirection::Down);
    let signed = if toward_second { delta } else { -delta };
    *ratio = (*ratio + signed).clamp(min, 1.0 - min);
    Some(())
}

fn subtree_contains(node: &LayoutNode, pane: Entity) -> bool {
    match node {
        LayoutNode::Leaf(e) => *e == pane,
        LayoutNode::Split { first, second, .. } => {
            subtree_contains(first, pane) || subtree_contains(second, pane)
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

    fn covers(rects: &[(Entity, PaneRect)], area: PaneRect) -> bool {
        let sum: f32 = rects.iter().map(|(_, r)| r.w * r.h).sum();
        sum <= area.w * area.h + 1.0
    }

    #[test]
    fn single_pane_fills_area() {
        let l = MultiplexerLayout::new(e(1));
        let area = PaneRect {
            x: 0.0,
            y: 0.0,
            w: 800.0,
            h: 600.0,
        };
        let rs = l.rects(area, 1.0);
        assert_eq!(rs, vec![(e(1), area)]);
    }

    #[test]
    fn vertical_split_side_by_side_with_gap() {
        let mut l = MultiplexerLayout::new(e(1));
        l.split(e(1), e(2), SplitAxis::Vertical);
        let area = PaneRect {
            x: 0.0,
            y: 0.0,
            w: 101.0,
            h: 50.0,
        };
        let rs = l.rects(area, 1.0);
        let left = rs.iter().find(|(x, _)| *x == e(1)).unwrap().1;
        let right = rs.iter().find(|(x, _)| *x == e(2)).unwrap().1;
        assert_eq!(left.x, 0.0);
        assert_eq!(left.w, 50.0);
        assert_eq!(right.x, 51.0);
        assert_eq!(right.w, 50.0);
        assert!(!overlap(left, right));
    }

    #[test]
    fn vertical_split_with_gap_covers_full_area() {
        let mut l = MultiplexerLayout::new(e(1));
        l.split(e(1), e(2), SplitAxis::Vertical);
        let area = PaneRect {
            x: 0.0,
            y: 0.0,
            w: 101.0,
            h: 50.0,
        };
        let rs = l.rects(area, 1.0);
        assert!(covers(&rs, area));
    }

    #[test]
    fn zoom_shows_only_zoomed_pane_at_full_area() {
        let mut l = MultiplexerLayout::new(e(1));
        l.split(e(1), e(2), SplitAxis::Vertical);
        l.set_zoom(Some(e(2)));
        assert_eq!(l.zoomed(), Some(e(2)));
        let area = PaneRect {
            x: 0.0,
            y: 0.0,
            w: 101.0,
            h: 50.0,
        };
        assert_eq!(l.rects(area, 1.0), vec![(e(2), area)]);
    }

    #[test]
    fn remove_clears_zoom_of_removed_pane() {
        let mut l = MultiplexerLayout::new(e(1));
        l.split(e(1), e(2), SplitAxis::Vertical);
        l.set_zoom(Some(e(1)));
        l.remove(e(1));
        assert_eq!(l.zoomed(), None);
    }

    fn overlap(a: PaneRect, b: PaneRect) -> bool {
        a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
    }

    #[test]
    fn remove_collapses_parent_and_returns_neighbor() {
        let mut l = MultiplexerLayout::new(e(1));
        l.split(e(1), e(2), SplitAxis::Vertical);
        let neighbor = l.remove(e(1));
        assert_eq!(neighbor, Some(e(2)));
        assert_eq!(l.leaves(), vec![e(2)]);
    }

    #[test]
    fn remove_last_leaf_returns_none() {
        let mut l = MultiplexerLayout::new(e(1));
        assert_eq!(l.remove(e(1)), None);
    }

    #[test]
    fn remove_nested_keeps_other_subtree() {
        let mut l = MultiplexerLayout::new(e(1));
        l.split(e(1), e(2), SplitAxis::Vertical);
        l.split(e(2), e(3), SplitAxis::Horizontal);
        let n = l.remove(e(2));
        assert_eq!(n, Some(e(3)));
        assert_eq!(l.leaves(), vec![e(1), e(3)]);
    }

    #[test]
    fn neighbor_right_of_left_pane() {
        let mut l = MultiplexerLayout::new(e(1));
        l.split(e(1), e(2), SplitAxis::Vertical);
        let area = PaneRect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 50.0,
        };
        assert_eq!(
            l.neighbor(e(1), PaneDirection::Right, area, 1.0),
            Some(e(2))
        );
        assert_eq!(l.neighbor(e(2), PaneDirection::Left, area, 1.0), Some(e(1)));
        assert_eq!(l.neighbor(e(1), PaneDirection::Up, area, 1.0), None);
    }

    #[test]
    fn resize_right_grows_left_pane() {
        let mut l = MultiplexerLayout::new(e(1));
        l.split(e(1), e(2), SplitAxis::Vertical);
        assert!(l.resize(e(1), PaneDirection::Right, 0.1, 0.1));
        let area = PaneRect {
            x: 0.0,
            y: 0.0,
            w: 101.0,
            h: 50.0,
        };
        let rs = l.rects(area, 1.0);
        let left = rs.iter().find(|(x, _)| *x == e(1)).unwrap().1;
        assert_eq!(left.w, 60.0);
    }

    #[test]
    fn resize_clamps_at_min() {
        let mut l = MultiplexerLayout::new(e(1));
        l.split(e(1), e(2), SplitAxis::Vertical);
        assert!(l.resize(e(1), PaneDirection::Left, 0.9, 0.1));
        let area = PaneRect {
            x: 0.0,
            y: 0.0,
            w: 101.0,
            h: 50.0,
        };
        let rs = l.rects(area, 1.0);
        let left = rs.iter().find(|(x, _)| *x == e(1)).unwrap().1;
        assert_eq!(left.w, 10.0);
    }

    #[test]
    fn resize_left_grows_right_pane() {
        let mut l = MultiplexerLayout::new(e(1));
        l.split(e(1), e(2), SplitAxis::Vertical);
        assert!(l.resize(e(2), PaneDirection::Left, 0.1, 0.1));
        let area = PaneRect {
            x: 0.0,
            y: 0.0,
            w: 101.0,
            h: 50.0,
        };
        let rs = l.rects(area, 1.0);
        let right = rs.iter().find(|(x, _)| *x == e(2)).unwrap().1;
        assert_eq!(right.w, 60.0);
    }

    #[test]
    fn resize_second_child_clamps_at_min() {
        let mut l = MultiplexerLayout::new(e(1));
        l.split(e(1), e(2), SplitAxis::Vertical);
        assert!(l.resize(e(2), PaneDirection::Left, 0.9, 0.1));
        let area = PaneRect {
            x: 0.0,
            y: 0.0,
            w: 101.0,
            h: 50.0,
        };
        let rs = l.rects(area, 1.0);
        let left = rs.iter().find(|(x, _)| *x == e(1)).unwrap().1;
        let right = rs.iter().find(|(x, _)| *x == e(2)).unwrap().1;
        assert_eq!(left.w, 10.0);
        assert_eq!(right.w, 90.0);
    }

    #[test]
    fn resize_edge_pane_is_noop() {
        let l0 = MultiplexerLayout::new(e(1));
        let mut l = l0.clone();
        assert!(!l.resize(e(1), PaneDirection::Right, 0.1, 0.1));
    }
}
