//! The cell-unit pane layout: a binary split tree whose leaves are
//! panes, solved into whole-window rectangles with one-cell separators.

use crate::protocol::{PaneDirection, PaneId, PaneRect, Separator, SplitId, SplitOrientation};
use orzma_vt::prelude::{GridSize, MIN_COLUMNS};
use std::cmp::Reverse;

/// The smallest rectangle a leaf is laid out in.
const LEAF_MIN: GridSize = GridSize {
    cols: MIN_COLUMNS,
    rows: 1,
};

/// The solved geometry of every pane and separator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Solved {
    /// The extent the panes tile: the window, widened per axis to the
    /// tree's minimum when the window is smaller.
    pub size: GridSize,
    /// Every pane's rectangle in whole-window cell coordinates.
    pub panes: Vec<PaneRect>,
    /// Every divider between adjacent panes.
    pub separators: Vec<Separator>,
}

impl Solved {
    /// The rectangle of `pane`, when it is in the tree.
    pub fn rect_of(&self, pane: PaneId) -> Option<PaneRect> {
        self.panes.iter().find(|r| r.pane == pane).copied()
    }
}

/// A split was refused because the target leaf cannot hold two minimum
/// leaves and a separator along the split axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SplitRefused;

/// A root insertion was refused because the tree already has a pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RootOccupied;

/// The split tree plus the activation history.
#[derive(Debug, Default)]
pub struct LayoutTree {
    root: Option<Node>,
    history: Vec<PaneId>,
    next_split_id: u32,
}

// TODO: make the drag minimum configurable.
const MIN_DRAG_COLS: u16 = 4;
const MIN_DRAG_ROWS: u16 = 2;

#[derive(Debug)]
enum Node {
    Leaf(PaneId),
    Split(Split),
}

#[derive(Debug)]
struct Split {
    id: SplitId,
    orientation: SplitOrientation,
    ratio: f32,
    first: Box<Node>,
    second: Box<Node>,
}

#[derive(Debug, Clone, Copy)]
struct Rect {
    x: u16,
    y: u16,
    cols: u16,
    rows: u16,
}

impl LayoutTree {
    /// An empty tree with no active pane.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the tree holds no pane.
    pub fn is_empty(&self) -> bool {
        self.root.is_none()
    }

    /// The active pane, if any: the most recently activated survivor.
    pub fn active(&self) -> Option<PaneId> {
        self.history.last().copied()
    }

    /// Every pane in the tree, left-to-right / top-to-bottom.
    pub fn panes(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        if let Some(root) = &self.root {
            root.collect_leaves(&mut out);
        }
        out
    }

    /// Makes `pane` the only pane. Refused while any pane exists.
    pub fn insert_root(&mut self, pane: PaneId) -> Result<(), RootOccupied> {
        if self.root.is_some() {
            return Err(RootOccupied);
        }
        self.root = Some(Node::Leaf(pane));
        self.activate(pane);
        Ok(())
    }

    /// Splits `target` in two, placing `new` right of / below it, and
    /// makes `new` active. Refused when `target` is missing or, along the
    /// axis in `window`, cannot hold two minimum leaves and a separator.
    pub fn split(
        &mut self,
        target: PaneId,
        orientation: SplitOrientation,
        new: PaneId,
        window: GridSize,
    ) -> Result<(), SplitRefused> {
        let rect = self.solve(window).rect_of(target).ok_or(SplitRefused)?;
        let (along, needed) = match orientation {
            SplitOrientation::Vertical => (rect.cols, 2 * LEAF_MIN.cols + 1),
            SplitOrientation::Horizontal => (rect.rows, 2 * LEAF_MIN.rows + 1),
        };
        if along < needed {
            return Err(SplitRefused);
        }
        let id = SplitId(self.next_split_id);
        self.next_split_id += 1;
        let Some(root) = self.root.as_mut() else {
            return Err(SplitRefused);
        };
        if !root.split_leaf(target, orientation, new, id) {
            return Err(SplitRefused);
        }
        self.activate(new);
        Ok(())
    }

    /// Removes `pane`, handing its space to its sibling. Returns whether
    /// it existed. A removed active pane is replaced by the most recently
    /// active survivor.
    pub fn remove(&mut self, pane: PaneId) -> bool {
        let Some(root) = self.root.take() else {
            return false;
        };
        let (next, removed) = root.without(pane);
        self.root = next;
        if removed {
            self.history.retain(|p| *p != pane);
        }
        removed
    }

    /// Makes `pane` active. Returns whether it exists.
    pub fn select(&mut self, pane: PaneId) -> bool {
        if !self.contains(pane) {
            return false;
        }
        self.activate(pane);
        true
    }

    /// Moves the active pane to its neighbour in `direction`: adjacent
    /// across one separator on the virtual (unclipped) geometry, with
    /// overlapping extent on the other axis; the most recently active
    /// candidate wins, never-visited candidates tie to the top / left.
    /// Returns whether the active pane changed.
    pub fn select_direction(&mut self, direction: PaneDirection, window: GridSize) -> bool {
        let Some(active) = self.active() else {
            return false;
        };
        let solved = self.solve(window);
        let Some(from) = solved.rect_of(active) else {
            return false;
        };
        let best = solved
            .panes
            .iter()
            .filter(|r| r.pane != active && adjacent(&from, r, direction))
            .max_by_key(|r| (self.recency(r.pane), Reverse((r.y, r.x))))
            .map(|r| r.pane);
        match best {
            Some(pane) => {
                self.activate(pane);
                true
            }
            None => false,
        }
    }

    /// Solves the tree against `window`, using `max(window, minimum)`
    /// per axis so every pane keeps at least one cell; the caller clips
    /// what overflows.
    pub fn solve(&self, window: GridSize) -> Solved {
        let mut solved = Solved {
            size: window,
            panes: Vec::new(),
            separators: Vec::new(),
        };
        let (Some(root), Some(rect)) = (&self.root, self.root_rect(window)) else {
            return solved;
        };
        solved.size = GridSize {
            cols: rect.cols,
            rows: rect.rows,
        };
        root.solve_into(&mut solved, rect);
        solved
    }

    /// Moves `split`'s divider to `position`, a whole-window cell
    /// boundary, clamped so neither side falls below the drag minimum.
    /// A window too small to honour that minimum falls back to the
    /// tree's own minimum. Returns whether the ratio changed; a `split`
    /// that is not in the tree returns `false`.
    pub fn resize_split(&mut self, split: SplitId, position: u16, window: GridSize) -> bool {
        let Some(root_rect) = self.root_rect(window) else {
            return false;
        };
        let Some(root) = self.root.as_mut() else {
            return false;
        };
        let Some((node, rect)) = root.find_split_mut(split, root_rect) else {
            return false;
        };
        let (avail, origin, drag_lo, drag_hi) = match node.orientation {
            SplitOrientation::Vertical => {
                let avail = rect.cols - 1;
                (
                    avail,
                    rect.x,
                    node.first.min_size_for_drag().cols,
                    avail.saturating_sub(node.second.min_size_for_drag().cols),
                )
            }
            SplitOrientation::Horizontal => {
                let avail = rect.rows - 1;
                (
                    avail,
                    rect.y,
                    node.first.min_size_for_drag().rows,
                    avail.saturating_sub(node.second.min_size_for_drag().rows),
                )
            }
        };
        let (lo, hi) = if drag_lo > drag_hi {
            match node.orientation {
                SplitOrientation::Vertical => (
                    node.first.min_size().cols,
                    avail - node.second.min_size().cols,
                ),
                SplitOrientation::Horizontal => (
                    node.first.min_size().rows,
                    avail - node.second.min_size().rows,
                ),
            }
        } else {
            (drag_lo, drag_hi)
        };
        let first = position.saturating_sub(origin).clamp(lo, hi);
        let ratio = f32::from(first) / f32::from(avail);
        if node.ratio == ratio {
            return false;
        }
        node.ratio = ratio;
        true
    }

    #[cfg(test)]
    fn set_root_ratio_for_test(&mut self, ratio: f32) {
        if let Some(Node::Split(split)) = self.root.as_mut() {
            split.ratio = ratio;
        }
    }

    /// Forgets every activation but the active pane's own.
    #[cfg(test)]
    fn clear_history_for_test(&mut self) {
        let active = self.history.pop();
        self.history.clear();
        self.history.extend(active);
    }

    #[cfg(test)]
    fn min_size_for_drag(&self) -> GridSize {
        self.root
            .as_ref()
            .map(Node::min_size_for_drag)
            .unwrap_or(GridSize { cols: 0, rows: 0 })
    }

    fn contains(&self, pane: PaneId) -> bool {
        self.root.as_ref().is_some_and(|root| root.has_leaf(pane))
    }

    fn activate(&mut self, pane: PaneId) {
        self.history.retain(|p| *p != pane);
        self.history.push(pane);
    }

    /// Position in the activation history (higher is more recent), or
    /// `None` for a pane never activated.
    fn recency(&self, pane: PaneId) -> Option<usize> {
        self.history.iter().position(|p| *p == pane)
    }

    /// The rectangle the tree tiles for `window`: the window widened per
    /// axis to the tree's minimum. `None` when the tree is empty.
    fn root_rect(&self, window: GridSize) -> Option<Rect> {
        let root = self.root.as_ref()?;
        let min = root.min_size();
        Some(Rect {
            x: 0,
            y: 0,
            cols: window.cols.max(min.cols),
            rows: window.rows.max(min.rows),
        })
    }
}

impl Node {
    fn has_leaf(&self, pane: PaneId) -> bool {
        match self {
            Node::Leaf(id) => *id == pane,
            Node::Split(s) => s.first.has_leaf(pane) || s.second.has_leaf(pane),
        }
    }

    fn collect_leaves(&self, out: &mut Vec<PaneId>) {
        match self {
            Node::Leaf(id) => out.push(*id),
            Node::Split(s) => {
                s.first.collect_leaves(out);
                s.second.collect_leaves(out);
            }
        }
    }

    /// Minimum size: a leaf is [`LEAF_MIN`]; a split needs both children
    /// plus one separator along its axis and the larger child across it.
    fn min_size(&self) -> GridSize {
        match self {
            Node::Leaf(_) => LEAF_MIN,
            Node::Split(s) => {
                let a = s.first.min_size();
                let b = s.second.min_size();
                match s.orientation {
                    SplitOrientation::Vertical => GridSize {
                        cols: a.cols + 1 + b.cols,
                        rows: a.rows.max(b.rows),
                    },
                    SplitOrientation::Horizontal => GridSize {
                        cols: a.cols.max(b.cols),
                        rows: a.rows + 1 + b.rows,
                    },
                }
            }
        }
    }

    /// Minimum size a drag may not shrink this subtree past: a leaf is
    /// `MIN_DRAG_COLS` × `MIN_DRAG_ROWS`; a split needs both children
    /// plus one separator along its axis and the larger child across it.
    fn min_size_for_drag(&self) -> GridSize {
        match self {
            Node::Leaf(_) => GridSize {
                cols: MIN_DRAG_COLS,
                rows: MIN_DRAG_ROWS,
            },
            Node::Split(s) => {
                let a = s.first.min_size_for_drag();
                let b = s.second.min_size_for_drag();
                match s.orientation {
                    SplitOrientation::Vertical => GridSize {
                        cols: a.cols + 1 + b.cols,
                        rows: a.rows.max(b.rows),
                    },
                    SplitOrientation::Horizontal => GridSize {
                        cols: a.cols.max(b.cols),
                        rows: a.rows + 1 + b.rows,
                    },
                }
            }
        }
    }

    fn solve_into(&self, out: &mut Solved, rect: Rect) {
        match self {
            Node::Leaf(id) => out.panes.push(PaneRect {
                pane: *id,
                x: rect.x,
                y: rect.y,
                cols: rect.cols,
                rows: rect.rows,
            }),
            Node::Split(s) => {
                let (first_rect, separator, second_rect) = s.subdivide(rect);
                s.first.solve_into(out, first_rect);
                out.separators.push(separator);
                s.second.solve_into(out, second_rect);
            }
        }
    }

    /// Replaces the leaf `target` with a half-and-half split whose second
    /// child is `new`. Returns whether the leaf was found.
    fn split_leaf(
        &mut self,
        target: PaneId,
        orientation: SplitOrientation,
        new: PaneId,
        id: SplitId,
    ) -> bool {
        match self {
            Node::Leaf(leaf) if *leaf == target => {
                *self = Node::Split(Split {
                    id,
                    orientation,
                    ratio: 0.5,
                    first: Box::new(Node::Leaf(target)),
                    second: Box::new(Node::Leaf(new)),
                });
                true
            }
            Node::Leaf(_) => false,
            Node::Split(s) => {
                s.first.split_leaf(target, orientation, new, id)
                    || s.second.split_leaf(target, orientation, new, id)
            }
        }
    }

    /// The tree without `pane`: the split immediately containing the
    /// removed leaf collapses into its sibling, while an ancestor split
    /// whose child only shrank keeps its structure around that child.
    /// Returns `(new subtree or None when emptied, removed)`.
    fn without(self, pane: PaneId) -> (Option<Node>, bool) {
        match self {
            Node::Leaf(id) if id == pane => (None, true),
            Node::Leaf(id) => (Some(Node::Leaf(id)), false),
            Node::Split(Split {
                id,
                orientation,
                ratio,
                first,
                second,
            }) => {
                let (first, removed_first) = first.without(pane);
                let (second, removed_second) = second.without(pane);
                let node = match (first, second) {
                    (Some(first), Some(second)) => Some(Node::Split(Split {
                        id,
                        orientation,
                        ratio,
                        first: Box::new(first),
                        second: Box::new(second),
                    })),
                    (Some(only), None) | (None, Some(only)) => Some(only),
                    (None, None) => None,
                };
                (node, removed_first || removed_second)
            }
        }
    }

    /// The split with `id` and the rectangle it divides, searched from
    /// `rect`.
    fn find_split_mut(&mut self, id: SplitId, rect: Rect) -> Option<(&mut Split, Rect)> {
        match self {
            Node::Leaf(_) => None,
            Node::Split(s) => {
                if s.id == id {
                    return Some((s, rect));
                }
                let (first_rect, _, second_rect) = s.subdivide(rect);
                s.first
                    .find_split_mut(id, first_rect)
                    .or_else(|| s.second.find_split_mut(id, second_rect))
            }
        }
    }
}

impl Split {
    /// The child rectangles `rect` divides into, and the divider between
    /// them.
    fn subdivide(&self, rect: Rect) -> (Rect, Separator, Rect) {
        let min_first = self.first.min_size();
        let min_second = self.second.min_size();
        match self.orientation {
            SplitOrientation::Vertical => {
                let avail = rect.cols - 1;
                let first = share(avail, self.ratio, min_first.cols, min_second.cols);
                (
                    Rect {
                        cols: first,
                        ..rect
                    },
                    Separator {
                        split: self.id,
                        orientation: SplitOrientation::Vertical,
                        x: rect.x + first,
                        y: rect.y,
                        len: rect.rows,
                    },
                    Rect {
                        x: rect.x + first + 1,
                        cols: avail - first,
                        ..rect
                    },
                )
            }
            SplitOrientation::Horizontal => {
                let avail = rect.rows - 1;
                let first = share(avail, self.ratio, min_first.rows, min_second.rows);
                (
                    Rect {
                        rows: first,
                        ..rect
                    },
                    Separator {
                        split: self.id,
                        orientation: SplitOrientation::Horizontal,
                        x: rect.x,
                        y: rect.y + first,
                        len: rect.cols,
                    },
                    Rect {
                        y: rect.y + first + 1,
                        rows: avail - first,
                        ..rect
                    },
                )
            }
        }
    }
}

/// `first`'s cells out of `avail`, rounded from `ratio` and clamped so
/// both children keep their minimum. `avail >= min_first + min_second`
/// must hold.
fn share(avail: u16, ratio: f32, min_first: u16, min_second: u16) -> u16 {
    let wanted = (f32::from(avail) * ratio).round() as u16;
    wanted.clamp(min_first, avail - min_second)
}

/// Whether `to` sits directly across one separator from `from` in
/// `direction` and overlaps it on the other axis.
fn adjacent(from: &PaneRect, to: &PaneRect, direction: PaneDirection) -> bool {
    let overlaps_rows = to.y < from.y + from.rows && from.y < to.y + to.rows;
    let overlaps_cols = to.x < from.x + from.cols && from.x < to.x + to.cols;
    match direction {
        PaneDirection::Left => overlaps_rows && to.x + to.cols + 1 == from.x,
        PaneDirection::Right => overlaps_rows && from.x + from.cols + 1 == to.x,
        PaneDirection::Up => overlaps_cols && to.y + to.rows + 1 == from.y,
        PaneDirection::Down => overlaps_cols && from.y + from.rows + 1 == to.y,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: GridSize = GridSize { cols: 80, rows: 24 };

    fn rect_of(solved: &Solved, pane: PaneId) -> PaneRect {
        solved.rect_of(pane).expect("pane rect")
    }

    fn two_side_by_side() -> LayoutTree {
        let mut tree = LayoutTree::new();
        tree.insert_root(PaneId(1)).unwrap();
        tree.split(PaneId(1), SplitOrientation::Vertical, PaneId(2), W)
            .unwrap();
        tree
    }

    /// Asserts that a vertical split halves the width around a one-cell
    /// separator and makes the new pane active.
    ///
    /// Case: the user presses split-vertical-pane on the only pane of an
    /// 80×24 window.
    #[test]
    fn a_vertical_split_halves_the_width_around_a_separator() {
        let tree = two_side_by_side();
        let solved = tree.solve(W);
        assert_eq!(
            rect_of(&solved, PaneId(1)),
            PaneRect {
                pane: PaneId(1),
                x: 0,
                y: 0,
                cols: 40,
                rows: 24
            }
        );
        assert_eq!(
            rect_of(&solved, PaneId(2)),
            PaneRect {
                pane: PaneId(2),
                x: 41,
                y: 0,
                cols: 39,
                rows: 24
            }
        );
        assert_eq!(
            solved.separators,
            vec![Separator {
                split: SplitId(0),
                orientation: SplitOrientation::Vertical,
                x: 40,
                y: 0,
                len: 24
            }]
        );
        assert_eq!(tree.active(), Some(PaneId(2)));
    }

    /// Asserts that a horizontal split stacks the panes with the new one
    /// below.
    ///
    /// Case: the user presses split-horizontal-pane on an 80×24 pane.
    #[test]
    fn a_horizontal_split_stacks_the_new_pane_below() {
        let mut tree = LayoutTree::new();
        tree.insert_root(PaneId(1)).unwrap();
        tree.split(PaneId(1), SplitOrientation::Horizontal, PaneId(2), W)
            .unwrap();
        let solved = tree.solve(W);
        assert_eq!(rect_of(&solved, PaneId(1)).rows, 12);
        assert_eq!(
            rect_of(&solved, PaneId(2)),
            PaneRect {
                pane: PaneId(2),
                x: 0,
                y: 13,
                cols: 80,
                rows: 11
            }
        );
    }

    /// Asserts that a leaf too narrow to hold two minimum leaves and a
    /// separator along the split axis refuses to split.
    ///
    /// Case: the user keeps splitting a pane until it is two columns wide.
    #[test]
    fn a_split_needs_room_for_two_minimum_leaves_and_a_separator() {
        let mut tree = LayoutTree::new();
        tree.insert_root(PaneId(1)).unwrap();
        let narrow = GridSize { cols: 2, rows: 24 };
        assert!(
            tree.split(PaneId(1), SplitOrientation::Vertical, PaneId(2), narrow)
                .is_err()
        );
        assert!(
            tree.split(PaneId(1), SplitOrientation::Horizontal, PaneId(2), narrow)
                .is_ok()
        );
    }

    /// Asserts that removing a pane hands its space to its sibling and
    /// re-activates the most recently active survivor.
    ///
    /// Case: the user kills the right pane of a two-pane window after
    /// having worked in the left one earlier.
    #[test]
    fn removing_a_pane_gives_its_space_to_the_sibling_and_restores_recency() {
        let mut tree = two_side_by_side();
        tree.select(PaneId(1));
        tree.select(PaneId(2));
        assert!(tree.remove(PaneId(2)));
        assert_eq!(tree.active(), Some(PaneId(1)));
        let solved = tree.solve(W);
        assert_eq!(rect_of(&solved, PaneId(1)).cols, 80);
        assert!(solved.separators.is_empty());
    }

    /// Asserts that removing a leaf can change a pane that was not its
    /// sibling.
    ///
    /// Case: a width-11 window holds `(A | B) | C` with the left subtree
    /// at ratio 0.1, and the user kills B.
    #[test]
    fn removing_a_leaf_can_resize_a_pane_outside_its_subtree() {
        let window = GridSize { cols: 11, rows: 5 };
        let mut tree = LayoutTree::new();
        tree.insert_root(PaneId(1)).unwrap();
        tree.split(PaneId(1), SplitOrientation::Vertical, PaneId(3), window)
            .unwrap();
        tree.split(PaneId(1), SplitOrientation::Vertical, PaneId(2), window)
            .unwrap();
        tree.set_root_ratio_for_test(0.1);
        let before = tree.solve(window);
        assert_eq!(rect_of(&before, PaneId(3)).cols, 5);
        tree.remove(PaneId(2));
        let after = tree.solve(window);
        assert_eq!(rect_of(&after, PaneId(1)).cols, 2);
        assert_eq!(rect_of(&after, PaneId(3)).cols, 8);
    }

    /// Asserts that directional selection picks the adjacent, overlapping
    /// neighbour and does nothing at the window edge.
    ///
    /// Case: the user presses select-right-pane from the left pane and
    /// then again from the right pane.
    #[test]
    fn select_direction_moves_to_the_adjacent_pane_and_stops_at_the_edge() {
        let mut tree = two_side_by_side();
        tree.select(PaneId(1));
        assert!(tree.select_direction(PaneDirection::Right, W));
        assert_eq!(tree.active(), Some(PaneId(2)));
        assert!(!tree.select_direction(PaneDirection::Right, W));
        assert_eq!(tree.active(), Some(PaneId(2)));
        assert!(tree.select_direction(PaneDirection::Left, W));
        assert_eq!(tree.active(), Some(PaneId(1)));
    }

    /// Asserts that among several adjacent candidates the most recently
    /// active one wins, and that never-visited ties fall to the topmost.
    ///
    /// Case: the right column is split into top and bottom; the user
    /// visited the bottom one, went left, and presses select-right again.
    #[test]
    fn select_direction_prefers_the_most_recently_active_candidate() {
        let mut tree = two_side_by_side();
        tree.split(PaneId(2), SplitOrientation::Horizontal, PaneId(3), W)
            .unwrap();
        tree.select(PaneId(1));
        assert!(tree.select_direction(PaneDirection::Right, W));
        assert_eq!(
            tree.active(),
            Some(PaneId(3)),
            "pane 3 was active most recently"
        );

        let mut fresh = two_side_by_side();
        fresh
            .split(PaneId(2), SplitOrientation::Horizontal, PaneId(3), W)
            .unwrap();
        fresh.clear_history_for_test();
        fresh.select(PaneId(1));
        fresh.clear_history_for_test();
        assert!(fresh.select_direction(PaneDirection::Right, W));
        assert_eq!(
            fresh.active(),
            Some(PaneId(2)),
            "never-visited candidates tie to the topmost"
        );
    }

    /// Asserts that a window smaller than the tree's minimum solves
    /// against the minimum, keeping every pane at its leaf minimum and
    /// overflowing the window instead of collapsing.
    ///
    /// Case: the user shrinks the window to one column while two panes
    /// sit side by side.
    #[test]
    fn a_window_below_the_minimum_solves_as_a_clipped_virtual_layout() {
        let tree = two_side_by_side();
        let solved = tree.solve(GridSize { cols: 1, rows: 1 });
        assert_eq!(
            rect_of(&solved, PaneId(1)),
            PaneRect {
                pane: PaneId(1),
                x: 0,
                y: 0,
                cols: 2,
                rows: 1
            }
        );
        assert_eq!(
            rect_of(&solved, PaneId(2)),
            PaneRect {
                pane: PaneId(2),
                x: 3,
                y: 0,
                cols: 2,
                rows: 1
            }
        );
    }

    /// Asserts that directional selection in a clipped layout still walks
    /// to the adjacent pane rather than jumping to a farther one.
    ///
    /// Case: three panes side by side in a window narrower than the tree;
    /// the user presses select-right from the leftmost.
    #[test]
    fn select_direction_does_not_skip_neighbours_when_clipped() {
        let mut tree = two_side_by_side();
        tree.split(PaneId(2), SplitOrientation::Vertical, PaneId(3), W)
            .unwrap();
        tree.select(PaneId(1));
        let tiny = GridSize { cols: 2, rows: 1 };
        assert!(tree.select_direction(PaneDirection::Right, tiny));
        assert_eq!(tree.active(), Some(PaneId(2)));
    }

    /// Asserts that `insert_root` is refused once a pane exists and that
    /// removing the last pane empties the tree.
    ///
    /// Case: the backend receives a stray `NewPane { Root }` while a pane
    /// is open, and later the last shell exits.
    #[test]
    fn root_insertion_is_exclusive_and_the_last_removal_empties_the_tree() {
        let mut tree = LayoutTree::new();
        tree.insert_root(PaneId(1)).unwrap();
        assert!(tree.insert_root(PaneId(2)).is_err());
        assert!(tree.remove(PaneId(1)));
        assert!(tree.is_empty());
        assert_eq!(tree.active(), None);
    }

    /// Asserts that a split that survives the collapse of an unrelated
    /// subtree keeps the id it was minted with.
    ///
    /// Case: the user splits the window vertically, splits the right
    /// half horizontally, splits that bottom pane again, then closes one
    /// of the innermost panes.
    #[test]
    fn an_ancestor_split_keeps_its_id_when_a_descendant_collapses() {
        let mut tree = LayoutTree::new();
        tree.insert_root(PaneId(1)).unwrap();
        tree.split(PaneId(1), SplitOrientation::Vertical, PaneId(2), W)
            .unwrap();
        tree.split(PaneId(2), SplitOrientation::Horizontal, PaneId(3), W)
            .unwrap();
        tree.split(PaneId(3), SplitOrientation::Horizontal, PaneId(4), W)
            .unwrap();
        let before: Vec<SplitId> = tree.solve(W).separators.iter().map(|s| s.split).collect();
        assert_eq!(before.len(), 3);

        assert!(tree.remove(PaneId(4)));

        let after: Vec<SplitId> = tree.solve(W).separators.iter().map(|s| s.split).collect();
        assert_eq!(after.len(), 2);
        for id in &after {
            assert!(before.contains(id), "{id:?} was renumbered by the collapse");
        }
    }

    /// Asserts that the id of a removed split is never handed to a later
    /// split.
    ///
    /// Case: the user splits a pane, closes the new pane, then splits
    /// again.
    #[test]
    fn a_split_id_is_never_reused() {
        let mut tree = LayoutTree::new();
        tree.insert_root(PaneId(1)).unwrap();
        tree.split(PaneId(1), SplitOrientation::Vertical, PaneId(2), W)
            .unwrap();
        let first = tree.solve(W).separators[0].split;

        assert!(tree.remove(PaneId(2)));
        assert!(tree.solve(W).separators.is_empty());

        tree.split(PaneId(1), SplitOrientation::Vertical, PaneId(3), W)
            .unwrap();
        assert_ne!(tree.solve(W).separators[0].split, first);
    }

    /// Asserts that the drag minimum of a column of N panes is
    /// `5N - 1` cells wide and that stacking N panes needs `3N - 1`
    /// rows, so the per-leaf minimum composes through nested splits.
    ///
    /// Case: the user has built a three-pane column and then a
    /// three-pane stack.
    #[test]
    fn the_drag_minimum_composes_through_nested_splits() {
        let mut columns = LayoutTree::new();
        columns.insert_root(PaneId(1)).unwrap();
        columns
            .split(PaneId(1), SplitOrientation::Vertical, PaneId(2), W)
            .unwrap();
        columns
            .split(PaneId(2), SplitOrientation::Vertical, PaneId(3), W)
            .unwrap();
        assert_eq!(columns.min_size_for_drag().cols, 14);

        let mut rows = LayoutTree::new();
        rows.insert_root(PaneId(1)).unwrap();
        rows.split(PaneId(1), SplitOrientation::Horizontal, PaneId(2), W)
            .unwrap();
        rows.split(PaneId(2), SplitOrientation::Horizontal, PaneId(3), W)
            .unwrap();
        assert_eq!(rows.min_size_for_drag().rows, 8);
    }

    /// Asserts that moving a divider puts it on the requested
    /// whole-window cell and that solving again reports it there, so the
    /// cell → ratio → cell round trip is exact.
    ///
    /// Case: the user drags the divider of a two-pane 80×24 window from
    /// the middle out to column 60.
    #[test]
    fn a_resize_puts_the_divider_on_the_requested_cell() {
        let mut tree = two_side_by_side();
        let split = tree.solve(W).separators[0].split;

        assert!(tree.resize_split(split, 60, W));

        let solved = tree.solve(W);
        assert_eq!(solved.separators[0].x, 60);
        assert_eq!(rect_of(&solved, PaneId(1)).cols, 60);
        assert_eq!(rect_of(&solved, PaneId(2)).cols, 19);
    }

    /// Asserts that a divider dragged past either end stops at the drag
    /// minimum rather than collapsing the pane behind it.
    ///
    /// Case: the user throws the divider of a two-pane 80×24 window all
    /// the way to the left edge, then all the way to the right.
    #[test]
    fn a_resize_clamps_to_the_drag_minimum_on_both_sides() {
        let mut tree = two_side_by_side();
        let split = tree.solve(W).separators[0].split;

        assert!(tree.resize_split(split, 0, W));
        assert_eq!(tree.solve(W).separators[0].x, 4);

        assert!(tree.resize_split(split, 79, W));
        assert_eq!(tree.solve(W).separators[0].x, 75);
    }

    /// Asserts that a divider dragged past either end of a stacked pair
    /// stops at the drag minimum in rows rather than in columns.
    ///
    /// Case: the user throws the divider of a two-pane 80×24 window up
    /// to the top edge, then all the way down to the bottom.
    #[test]
    fn a_horizontal_resize_clamps_to_the_drag_minimum_on_both_sides() {
        let mut tree = LayoutTree::new();
        tree.insert_root(PaneId(1)).unwrap();
        tree.split(PaneId(1), SplitOrientation::Horizontal, PaneId(2), W)
            .unwrap();
        let split = tree.solve(W).separators[0].split;

        assert!(tree.resize_split(split, 0, W));
        assert_eq!(tree.solve(W).separators[0].y, 2);

        assert!(tree.resize_split(split, 23, W));
        assert_eq!(tree.solve(W).separators[0].y, 21);
    }

    /// Asserts that a resize addressed to an id no longer in the tree is
    /// refused and changes nothing.
    ///
    /// Case: the shell in one pane exits mid-drag, collapsing the split
    /// the pointer was holding, and the next drag frame still names it.
    #[test]
    fn a_resize_of_an_unknown_split_is_refused() {
        let mut tree = two_side_by_side();
        let before = tree.solve(W);

        assert!(!tree.resize_split(SplitId(999), 60, W));

        assert_eq!(tree.solve(W), before);
    }

    /// Asserts that a position left of the split's own origin saturates
    /// to the drag minimum instead of underflowing.
    ///
    /// Case: the user drags the right-hand divider of a nested layout
    /// far past the left edge of the window.
    #[test]
    fn a_position_before_the_split_origin_saturates() {
        let mut tree = LayoutTree::new();
        tree.insert_root(PaneId(1)).unwrap();
        tree.split(PaneId(1), SplitOrientation::Vertical, PaneId(2), W)
            .unwrap();
        tree.split(PaneId(2), SplitOrientation::Vertical, PaneId(3), W)
            .unwrap();
        let inner = tree.solve(W).separators[1].split;

        assert!(tree.resize_split(inner, 0, W));

        let solved = tree.solve(W);
        assert_eq!(solved.separators[1].x, 45);
    }

    /// Asserts that resizing an inner split leaves the outer divider
    /// where it was.
    ///
    /// Case: the user drags the divider inside the right-hand column of
    /// a two-column layout.
    #[test]
    fn resizing_an_inner_split_leaves_the_outer_divider() {
        let mut tree = LayoutTree::new();
        tree.insert_root(PaneId(1)).unwrap();
        tree.split(PaneId(1), SplitOrientation::Vertical, PaneId(2), W)
            .unwrap();
        tree.split(PaneId(2), SplitOrientation::Horizontal, PaneId(3), W)
            .unwrap();
        let outer_x = tree.solve(W).separators[0].x;
        let inner = tree.solve(W).separators[1].split;

        assert!(tree.resize_split(inner, 6, W));

        let solved = tree.solve(W);
        assert_eq!(solved.separators[0].x, outer_x);
        assert_eq!(solved.separators[1].y, 6);
    }

    /// Asserts that a window too narrow to honour the drag minimum on
    /// both sides still lets the divider move, falling back to the
    /// tree's own two-column leaf minimum.
    ///
    /// Case: the user shrinks the window until a two-pane row no
    /// longer fits its drag minimum, then drags a divider anyway.
    #[test]
    fn a_cramped_window_falls_back_to_the_tree_minimum() {
        let narrow = GridSize { cols: 8, rows: 24 };
        let mut tree = LayoutTree::new();
        tree.insert_root(PaneId(1)).unwrap();
        tree.split(PaneId(1), SplitOrientation::Vertical, PaneId(2), narrow)
            .unwrap();
        let split = tree.solve(narrow).separators[0].split;

        assert!(tree.resize_split(split, 1, narrow));

        assert_eq!(tree.solve(narrow).separators[0].x, 2);
    }

    /// Asserts that resizing against a window smaller than the one the
    /// layout was built for neither panics nor leaves the divider
    /// outside the solved extent.
    ///
    /// Case: the user drags a divider while dragging the window's own
    /// resize corner, so a smaller window arrives mid-drag.
    #[test]
    fn a_resize_against_a_shrunken_window_is_clamped() {
        let mut tree = two_side_by_side();
        let split = tree.solve(W).separators[0].split;
        let shrunk = GridSize { cols: 20, rows: 10 };

        assert!(tree.resize_split(split, 18, shrunk));

        let solved = tree.solve(shrunk);
        assert_eq!(solved.separators[0].x, 15);
    }

    /// Asserts that a column deeper than the window's drag minimum still
    /// moves its divider instead of panicking.
    ///
    /// Case: the user has four columns open and shrinks the window until
    /// the per-leaf drag minimum no longer fits, then drags a divider.
    #[test]
    fn a_deep_column_below_the_drag_minimum_still_resizes() {
        let wide = GridSize { cols: 23, rows: 24 };
        let narrow = GridSize { cols: 16, rows: 24 };
        let mut tree = LayoutTree::new();
        tree.insert_root(PaneId(1)).unwrap();
        for (target, new) in [(1, 2), (2, 3), (3, 4)] {
            tree.split(
                PaneId(target),
                SplitOrientation::Vertical,
                PaneId(new),
                wide,
            )
            .unwrap();
        }
        let split = tree.solve(narrow).separators[0].split;

        assert!(tree.resize_split(split, 14, narrow));

        let solved = tree.solve(narrow);
        assert_eq!(solved.separators[0].x, 7);
    }
}
