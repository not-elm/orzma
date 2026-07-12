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
        layout_rects(&self.root, area, gap, &mut out);
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

fn layout_rects(node: &LayoutNode, area: PaneRect, gap: f32, out: &mut Vec<(Entity, PaneRect)>) {
    match node {
        LayoutNode::Leaf(e) => out.push((*e, area)),
        LayoutNode::Split {
            axis,
            ratio,
            first,
            second,
        } => {
            let (a, b) = split_rect(area, *axis, *ratio, gap);
            layout_rects(first, a, gap, out);
            layout_rects(second, b, gap, out);
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

    fn overlap(a: PaneRect, b: PaneRect) -> bool {
        a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
    }
}
