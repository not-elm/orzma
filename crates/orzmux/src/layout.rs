//! The cell-unit pane layout: a binary split tree whose leaves are
//! panes, solved into whole-window rectangles with one-cell separators.

use crate::protocol::{PaneDirection, PaneId, PaneRect, Separator, SplitOrientation};
use orzma_vt::prelude::GridSize;
use std::cmp::Reverse;

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

/// A split was refused because the target leaf is narrower than three
/// cells along the split axis.
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
}

#[derive(Debug)]
enum Node {
    Leaf(PaneId),
    Split(Split),
}

#[derive(Debug)]
struct Split {
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
    /// makes `new` active. Refused when `target` is missing or narrower
    /// than three cells along the axis in `window`.
    pub fn split(
        &mut self,
        target: PaneId,
        orientation: SplitOrientation,
        new: PaneId,
        window: GridSize,
    ) -> Result<(), SplitRefused> {
        let rect = self.solve(window).rect_of(target).ok_or(SplitRefused)?;
        let along = match orientation {
            SplitOrientation::Vertical => rect.cols,
            SplitOrientation::Horizontal => rect.rows,
        };
        if along < 3 {
            return Err(SplitRefused);
        }
        let Some(root) = self.root.as_mut() else {
            return Err(SplitRefused);
        };
        if !root.split_leaf(target, orientation, new) {
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
        let Some(root) = &self.root else {
            return solved;
        };
        let min = root.min_size();
        solved.size = GridSize {
            cols: window.cols.max(min.cols),
            rows: window.rows.max(min.rows),
        };
        let rect = Rect {
            x: 0,
            y: 0,
            cols: solved.size.cols,
            rows: solved.size.rows,
        };
        root.solve_into(&mut solved, rect);
        solved
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

    /// Minimum size: a leaf is 1×1; a split needs both children plus one
    /// separator along its axis and the larger child across it.
    fn min_size(&self) -> GridSize {
        match self {
            Node::Leaf(_) => GridSize { cols: 1, rows: 1 },
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
                let min_first = s.first.min_size();
                let min_second = s.second.min_size();
                match s.orientation {
                    SplitOrientation::Vertical => {
                        let avail = rect.cols - 1;
                        let first = share(avail, s.ratio, min_first.cols, min_second.cols);
                        s.first.solve_into(
                            out,
                            Rect {
                                cols: first,
                                ..rect
                            },
                        );
                        out.separators.push(Separator {
                            orientation: SplitOrientation::Vertical,
                            x: rect.x + first,
                            y: rect.y,
                            len: rect.rows,
                        });
                        s.second.solve_into(
                            out,
                            Rect {
                                x: rect.x + first + 1,
                                cols: avail - first,
                                ..rect
                            },
                        );
                    }
                    SplitOrientation::Horizontal => {
                        let avail = rect.rows - 1;
                        let first = share(avail, s.ratio, min_first.rows, min_second.rows);
                        s.first.solve_into(
                            out,
                            Rect {
                                rows: first,
                                ..rect
                            },
                        );
                        out.separators.push(Separator {
                            orientation: SplitOrientation::Horizontal,
                            x: rect.x,
                            y: rect.y + first,
                            len: rect.cols,
                        });
                        s.second.solve_into(
                            out,
                            Rect {
                                y: rect.y + first + 1,
                                rows: avail - first,
                                ..rect
                            },
                        );
                    }
                }
            }
        }
    }

    /// Replaces the leaf `target` with a half-and-half split whose second
    /// child is `new`. Returns whether the leaf was found.
    fn split_leaf(&mut self, target: PaneId, orientation: SplitOrientation, new: PaneId) -> bool {
        match self {
            Node::Leaf(id) if *id == target => {
                *self = Node::Split(Split {
                    orientation,
                    ratio: 0.5,
                    first: Box::new(Node::Leaf(target)),
                    second: Box::new(Node::Leaf(new)),
                });
                true
            }
            Node::Leaf(_) => false,
            Node::Split(s) => {
                s.first.split_leaf(target, orientation, new)
                    || s.second.split_leaf(target, orientation, new)
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
                orientation,
                ratio,
                first,
                second,
            }) => {
                let (first, removed_first) = first.without(pane);
                let (second, removed_second) = second.without(pane);
                let node = match (first, second) {
                    (Some(first), Some(second)) => Some(Node::Split(Split {
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

    /// Asserts that a leaf narrower than three cells along the split axis
    /// refuses to split.
    ///
    /// Case: the user keeps splitting a pane until it is two columns wide.
    #[test]
    fn a_split_needs_three_cells_along_its_axis() {
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
        assert_eq!(rect_of(&before, PaneId(3)).cols, 7);
        tree.remove(PaneId(2));
        let after = tree.solve(window);
        assert_eq!(rect_of(&after, PaneId(1)).cols, 1);
        assert_eq!(rect_of(&after, PaneId(3)).cols, 9);
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
    /// against the minimum, keeping every pane at least 1×1 and
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
                cols: 1,
                rows: 1
            }
        );
        assert_eq!(
            rect_of(&solved, PaneId(2)),
            PaneRect {
                pane: PaneId(2),
                x: 2,
                y: 0,
                cols: 1,
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
}
