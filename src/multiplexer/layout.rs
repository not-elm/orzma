//! Bevy-free ratio binary layout tree for multiplexer panes.
//!
//! A window's panes form a binary tree of ratio-based splits. All geometry
//! here is pure and unit-tested; the Bevy layer converts `rects` output into
//! per-pane `Node`s and PTY sizes.

use bevy::ecs::entity::Entity;
use bevy::math::Vec2;
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

/// One step in a root-to-`Split` path: which child a divider's descent
/// takes at that `Split` node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChildSide {
    /// Descend into the split's `first` child.
    First,
    /// Descend into the split's `second` child.
    Second,
}

/// A divider's geometry: the 1px gap rect between a split's two children,
/// keyed by the root-to-`Split` path so nested same-axis splits resolve
/// unambiguously (see `MultiplexerLayout::divider_rects`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DividerRect {
    pub axis: SplitAxis,
    pub path: Vec<ChildSide>,
    pub rect: PaneRect,
    pub axis_extent: f32,
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

    /// Every leaf's display state: `Some(rect)` where the pane is visible,
    /// `None` where it is hidden (a non-zoomed pane while another pane is
    /// zoomed).
    pub(crate) fn display_rects(
        &self,
        area: PaneRect,
        gap: f32,
    ) -> Vec<(Entity, Option<PaneRect>)> {
        let visible = self.rects(area, gap);
        let mut out: Vec<(Entity, Option<PaneRect>)> =
            visible.into_iter().map(|(e, r)| (e, Some(r))).collect();
        for leaf in self.leaves() {
            if !out.iter().any(|(e, _)| *e == leaf) {
                out.push((leaf, None));
            }
        }
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

    /// Every divider's geometry: the 1px gap rect between a split's two
    /// children, keyed by the root-to-`Split` path so nested same-axis
    /// splits resolve unambiguously. Empty when a pane is zoomed (a single
    /// full-area pane has no dividers).
    pub(crate) fn divider_rects(&self, area: PaneRect, gap: f32) -> Vec<DividerRect> {
        if let Some(z) = self.zoomed
            && self.contains(z)
        {
            return Vec::new();
        }
        let mut out = Vec::new();
        collect_divider_rects(&mut out, &self.root, area, gap, Vec::new());
        out
    }

    /// Adjusts the ratio of the `Split` node at `path` by `delta_frac`,
    /// clamped to `[min_frac, 1.0 - min_frac]`. Positive `delta_frac` grows
    /// the FIRST child (moves the divider toward `second`); negative grows
    /// the second. Returns false (no panic, no change) when `path` does not
    /// resolve to a `Split` node, or the clamped ratio is unchanged.
    pub(crate) fn resize_split_at(
        &mut self,
        path: &[ChildSide],
        delta_frac: f32,
        min_frac: f32,
    ) -> bool {
        resize_split_in(&mut self.root, path, delta_frac, min_frac)
    }
}

/// Returns the index of the divider in `dividers` whose grab zone contains
/// `cursor` (same physical-px space as `divider_rects`), given a tolerance
/// `tol` in that space. The grab zone is the divider's gap rect expanded by
/// `tol` on the major axis, intersected with its span on the perpendicular
/// axis.
pub(crate) fn divider_at(dividers: &[DividerRect], cursor: Vec2, tol: f32) -> Option<usize> {
    dividers.iter().position(|d| match d.axis {
        SplitAxis::Vertical => {
            cursor.x >= d.rect.x - tol
                && cursor.x <= d.rect.x + d.rect.w + tol
                && cursor.y >= d.rect.y
                && cursor.y < d.rect.y + d.rect.h
        }
        SplitAxis::Horizontal => {
            cursor.y >= d.rect.y - tol
                && cursor.y <= d.rect.y + d.rect.h + tol
                && cursor.x >= d.rect.x
                && cursor.x < d.rect.x + d.rect.w
        }
    })
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
    let (first, _gap, second) = split_regions(area, axis, ratio, gap);
    (first, second)
}

fn split_regions(
    area: PaneRect,
    axis: SplitAxis,
    ratio: f32,
    gap: f32,
) -> (PaneRect, PaneRect, PaneRect) {
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
                    x: area.x + fw,
                    y: area.y,
                    w: gap,
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
                    y: area.y + fh,
                    w: area.w,
                    h: gap,
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

fn collect_divider_rects(
    out: &mut Vec<DividerRect>,
    node: &LayoutNode,
    area: PaneRect,
    gap: f32,
    path: Vec<ChildSide>,
) {
    let LayoutNode::Split {
        axis,
        ratio,
        first,
        second,
    } = node
    else {
        return;
    };
    let (first_area, gap_rect, second_area) = split_regions(area, *axis, *ratio, gap);
    let axis_extent = match axis {
        SplitAxis::Vertical => area.w - gap,
        SplitAxis::Horizontal => area.h - gap,
    };
    out.push(DividerRect {
        axis: *axis,
        path: path.clone(),
        rect: gap_rect,
        axis_extent,
    });
    let mut into_first = path.clone();
    into_first.push(ChildSide::First);
    collect_divider_rects(out, first, first_area, gap, into_first);
    let mut into_second = path;
    into_second.push(ChildSide::Second);
    collect_divider_rects(out, second, second_area, gap, into_second);
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
    let next = (*ratio + signed).clamp(min, 1.0 - min);
    if next == *ratio {
        return None;
    }
    *ratio = next;
    Some(())
}

fn resize_split_in(node: &mut LayoutNode, path: &[ChildSide], delta: f32, min: f32) -> bool {
    let Some((side, rest)) = path.split_first() else {
        let LayoutNode::Split { ratio, .. } = node else {
            return false;
        };
        let next = (*ratio + delta).clamp(min, 1.0 - min);
        if next == *ratio {
            return false;
        }
        *ratio = next;
        return true;
    };
    let LayoutNode::Split { first, second, .. } = node else {
        return false;
    };
    match side {
        ChildSide::First => resize_split_in(first, rest, delta, min),
        ChildSide::Second => resize_split_in(second, rest, delta, min),
    }
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
    fn display_rects_zoom_hides_siblings() {
        let mut l = MultiplexerLayout::new(e(1));
        l.split(e(1), e(2), SplitAxis::Vertical);
        l.set_zoom(Some(e(2)));
        let area = PaneRect {
            x: 0.0,
            y: 0.0,
            w: 101.0,
            h: 50.0,
        };
        let display = l.display_rects(area, 1.0);
        assert_eq!(display.len(), 2, "every leaf must have a display entry");
        let zoomed = display.iter().find(|(pane, _)| *pane == e(2)).unwrap().1;
        assert_eq!(
            zoomed,
            Some(area),
            "the zoomed pane must be visible at full area"
        );
        let hidden = display.iter().find(|(pane, _)| *pane == e(1)).unwrap().1;
        assert_eq!(hidden, None, "the non-zoomed sibling must be hidden");
    }

    #[test]
    fn display_rects_no_zoom_shows_every_leaf() {
        let mut l = MultiplexerLayout::new(e(1));
        l.split(e(1), e(2), SplitAxis::Vertical);
        let area = PaneRect {
            x: 0.0,
            y: 0.0,
            w: 101.0,
            h: 50.0,
        };
        let display = l.display_rects(area, 1.0);
        assert_eq!(display.len(), 2);
        assert!(
            display.iter().all(|(_, r)| r.is_some()),
            "with no zoom, every leaf must be visible"
        );
    }

    #[test]
    fn display_rects_nested_three_leaf_zoom_hides_other_two() {
        let mut l = MultiplexerLayout::new(e(1));
        l.split(e(1), e(2), SplitAxis::Vertical);
        l.split(e(2), e(3), SplitAxis::Horizontal);
        l.set_zoom(Some(e(3)));
        let area = PaneRect {
            x: 0.0,
            y: 0.0,
            w: 101.0,
            h: 101.0,
        };
        let display = l.display_rects(area, 1.0);
        assert_eq!(display.len(), 3, "every leaf must have a display entry");
        let zoomed = display.iter().find(|(pane, _)| *pane == e(3)).unwrap().1;
        assert_eq!(
            zoomed,
            Some(area),
            "the zoomed pane must be visible at full area"
        );
        let hidden_1 = display.iter().find(|(pane, _)| *pane == e(1)).unwrap().1;
        let hidden_2 = display.iter().find(|(pane, _)| *pane == e(2)).unwrap().1;
        assert_eq!(
            hidden_1, None,
            "a non-zoomed leaf in a nested split must be hidden"
        );
        assert_eq!(
            hidden_2, None,
            "a non-zoomed leaf in a nested split must be hidden"
        );
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

    #[test]
    fn divider_rects_single_vertical_split() {
        let mut l = MultiplexerLayout::new(e(1));
        l.split(e(1), e(2), SplitAxis::Vertical);
        let area = PaneRect {
            x: 0.0,
            y: 0.0,
            w: 101.0,
            h: 50.0,
        };
        let ds = l.divider_rects(area, 1.0);
        assert_eq!(ds.len(), 1);
        let d = &ds[0];
        assert_eq!(d.axis, SplitAxis::Vertical);
        assert_eq!(d.path, Vec::new());
        assert_eq!(
            d.rect,
            PaneRect {
                x: 50.0,
                y: 0.0,
                w: 1.0,
                h: 50.0
            }
        );
        assert_eq!(d.axis_extent, 100.0);
    }

    #[test]
    fn divider_rects_nested_layout_has_distinct_paths() {
        let mut l = MultiplexerLayout::new(e(1));
        l.split(e(1), e(2), SplitAxis::Vertical);
        l.split(e(2), e(3), SplitAxis::Horizontal);
        let area = PaneRect {
            x: 0.0,
            y: 0.0,
            w: 101.0,
            h: 101.0,
        };
        let ds = l.divider_rects(area, 1.0);
        assert_eq!(ds.len(), 2);
        let root = ds.iter().find(|d| d.path.is_empty()).unwrap();
        assert_eq!(root.axis, SplitAxis::Vertical);
        assert_eq!(
            root.rect,
            PaneRect {
                x: 50.0,
                y: 0.0,
                w: 1.0,
                h: 101.0
            }
        );
        assert_eq!(root.axis_extent, 100.0);
        let nested = ds.iter().find(|d| !d.path.is_empty()).unwrap();
        assert_eq!(nested.path, vec![ChildSide::Second]);
        assert_eq!(nested.axis, SplitAxis::Horizontal);
        assert_eq!(
            nested.rect,
            PaneRect {
                x: 51.0,
                y: 50.0,
                w: 50.0,
                h: 1.0
            }
        );
        assert_eq!(nested.axis_extent, 100.0);
        assert_ne!(
            root.path, nested.path,
            "each split must key a distinct divider"
        );
    }

    #[test]
    fn divider_rects_empty_when_zoomed() {
        let mut l = MultiplexerLayout::new(e(1));
        l.split(e(1), e(2), SplitAxis::Vertical);
        l.set_zoom(Some(e(2)));
        let area = PaneRect {
            x: 0.0,
            y: 0.0,
            w: 101.0,
            h: 50.0,
        };
        assert!(l.divider_rects(area, 1.0).is_empty());
    }

    #[test]
    fn resize_split_at_positive_delta_grows_first_child() {
        let mut l = MultiplexerLayout::new(e(1));
        l.split(e(1), e(2), SplitAxis::Vertical);
        assert!(l.resize_split_at(&[], 0.1, 0.1));
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
    fn resize_split_at_negative_delta_grows_second_child() {
        let mut l = MultiplexerLayout::new(e(1));
        l.split(e(1), e(2), SplitAxis::Vertical);
        assert!(l.resize_split_at(&[], -0.1, 0.1));
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
    fn resize_split_at_clamps_at_max_then_is_noop() {
        let mut l = MultiplexerLayout::new(e(1));
        l.split(e(1), e(2), SplitAxis::Vertical);
        assert!(l.resize_split_at(&[], 0.9, 0.1));
        let area = PaneRect {
            x: 0.0,
            y: 0.0,
            w: 101.0,
            h: 50.0,
        };
        let rs = l.rects(area, 1.0);
        let left = rs.iter().find(|(x, _)| *x == e(1)).unwrap().1;
        assert_eq!(left.w, 90.0);
        assert!(!l.resize_split_at(&[], 0.9, 0.1));
    }

    #[test]
    fn resize_split_at_clamps_at_min_then_is_noop() {
        let mut l = MultiplexerLayout::new(e(1));
        l.split(e(1), e(2), SplitAxis::Vertical);
        assert!(l.resize_split_at(&[], -0.9, 0.1));
        let area = PaneRect {
            x: 0.0,
            y: 0.0,
            w: 101.0,
            h: 50.0,
        };
        let rs = l.rects(area, 1.0);
        let left = rs.iter().find(|(x, _)| *x == e(1)).unwrap().1;
        assert_eq!(left.w, 10.0);
        assert!(!l.resize_split_at(&[], -0.9, 0.1));
    }

    #[test]
    fn resize_split_at_bad_path_is_noop() {
        let mut l = MultiplexerLayout::new(e(1));
        l.split(e(1), e(2), SplitAxis::Vertical);
        assert!(!l.resize_split_at(&[ChildSide::First], 0.1, 0.1));
        assert!(!l.resize_split_at(&[ChildSide::First, ChildSide::Second], 0.1, 0.1));
    }

    fn div(axis: SplitAxis, rect: PaneRect) -> DividerRect {
        DividerRect {
            axis,
            path: Vec::new(),
            rect,
            axis_extent: 0.0,
        }
    }

    #[test]
    fn divider_at_hits_within_tolerance() {
        let d = div(
            SplitAxis::Vertical,
            PaneRect {
                x: 50.0,
                y: 0.0,
                w: 1.0,
                h: 50.0,
            },
        );
        let dividers = vec![d];
        assert_eq!(divider_at(&dividers, Vec2::new(50.5, 25.0), 2.0), Some(0));
        assert_eq!(divider_at(&dividers, Vec2::new(48.0, 25.0), 2.0), Some(0));
    }

    #[test]
    fn divider_at_misses_outside_perpendicular_span() {
        let d = div(
            SplitAxis::Vertical,
            PaneRect {
                x: 50.0,
                y: 0.0,
                w: 1.0,
                h: 50.0,
            },
        );
        let dividers = vec![d];
        assert_eq!(divider_at(&dividers, Vec2::new(50.5, 60.0), 2.0), None);
    }

    #[test]
    fn divider_at_misses_outside_major_axis_zone() {
        let d = div(
            SplitAxis::Vertical,
            PaneRect {
                x: 50.0,
                y: 0.0,
                w: 1.0,
                h: 50.0,
            },
        );
        let dividers = vec![d];
        assert_eq!(divider_at(&dividers, Vec2::new(60.0, 25.0), 2.0), None);
    }

    #[test]
    fn divider_at_returns_correct_index_among_many() {
        let d0 = div(
            SplitAxis::Vertical,
            PaneRect {
                x: 50.0,
                y: 0.0,
                w: 1.0,
                h: 50.0,
            },
        );
        let d1 = div(
            SplitAxis::Horizontal,
            PaneRect {
                x: 0.0,
                y: 80.0,
                w: 100.0,
                h: 1.0,
            },
        );
        let dividers = vec![d0, d1];
        assert_eq!(divider_at(&dividers, Vec2::new(40.0, 80.5), 2.0), Some(1));
    }
}
