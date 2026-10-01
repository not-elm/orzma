//! The cell-unit pane layout: a binary split tree whose leaves are
//! panes, tiled into whole-window rectangles with one-cell separators.

use crate::backend::{PaneDirection, PaneId, PaneRect, Separator, SplitId, SplitOrientation};
use crate::error::{OrzmuxError, OrzmuxResult};
use orzma_vt::prelude::{GridSize, MIN_COLUMNS};
use std::cmp::Reverse;

/// The smallest rectangle a leaf is laid out in.
const LEAF_MIN: GridSize = GridSize {
    cols: MIN_COLUMNS,
    rows: 1,
};

// TODO: make the drag minimum configurable.
const MIN_DRAG_COLS: u16 = 4;
const MIN_DRAG_ROWS: u16 = 2;

/// The geometry of every pane and separator of a tiled tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tiling {
    /// The extent the panes tile: the window, widened per axis to the
    /// tree's minimum when the window is smaller.
    pub size: GridSize,
    /// Every pane's rectangle in whole-window cell coordinates.
    pub panes: Vec<PaneRect>,
    /// Every divider between adjacent panes.
    pub separators: Vec<Separator>,
}

impl Tiling {
    /// The rectangle of `pane`, when it is in the tree.
    pub fn rect_of(&self, pane: PaneId) -> Option<PaneRect> {
        self.panes.iter().find(|r| r.pane == pane).copied()
    }

    /// Whether `pane`'s rectangle can hold two minimum leaves and a
    /// separator along the axis a split of `orientation` divides. It is
    /// `false` for a pane that is not in the tiling.
    pub fn can_split(&self, pane: PaneId, orientation: SplitOrientation) -> bool {
        self.rect_of(pane).is_some_and(|rect| match orientation {
            SplitOrientation::Vertical => rect.cols > 2 * LEAF_MIN.cols,
            SplitOrientation::Horizontal => rect.rows > 2 * LEAF_MIN.rows,
        })
    }
}

/// Mints split ids that stay unique across every tree it serves.
#[derive(Debug, Default)]
pub struct SplitIds {
    next: u32,
}

impl SplitIds {
    /// The next unused split id.
    pub fn mint(&mut self) -> SplitId {
        let id = SplitId(self.next);
        self.next += 1;
        id
    }
}

/// The split tree plus the activation history.
///
/// # Invariants
///
/// The tree holds at least one pane, and the active pane is one of them.
/// `history` holds the other panes activated earlier, oldest first, each
/// once and never the active pane.
#[derive(Debug)]
pub struct LayoutTree {
    root: Node,
    active: PaneId,
    history: Vec<PaneId>,
}

/// What [`LayoutTree::remove`] did; each variant carries the tree back.
#[derive(Debug)]
pub enum Removal {
    /// The pane is not in the tree, which is unchanged.
    Absent(LayoutTree),
    /// The pane left the tree, and its space went to its sibling.
    Removed(LayoutTree),
    /// The pane is the tree's only pane, and the tree is unchanged.
    Last(LayoutTree),
}

/// What [`Node::without`] left of a subtree.
enum Pruned {
    /// The pane is not in the subtree, which comes back unchanged.
    Absent(Node),
    /// The pane left the subtree; the survivors remain.
    Removed(Node),
    /// The subtree is the pane's own leaf, which comes back unchanged.
    Only(Node),
}

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

/// Where a split's divider may sit within the rectangle the split
/// divides, measured along the split's axis.
#[derive(Debug, Clone, Copy)]
struct DividerRange {
    /// The cells both children share: the extent minus the divider.
    avail: u16,
    /// The rectangle's first cell along the axis.
    origin: u16,
    /// The smallest first-child extent the divider may leave.
    lo: u16,
    /// The largest first-child extent the divider may leave.
    hi: u16,
}

/// One split on the path from a pane up to the root.
#[derive(Debug, Clone, Copy)]
struct Ancestor {
    split: SplitId,
    orientation: SplitOrientation,
    /// Whether the pane lies in the split's first child.
    in_first: bool,
}

impl LayoutTree {
    /// A tree whose only pane is `pane`, which is active.
    pub fn with_root(pane: PaneId) -> Self {
        Self {
            root: Node::Leaf(pane),
            active: pane,
            history: Vec::new(),
        }
    }

    /// The active pane: the most recently activated survivor.
    pub fn active(&self) -> PaneId {
        self.active
    }

    /// Every pane in the tree, left-to-right / top-to-bottom.
    pub fn panes(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        self.root.collect_leaves(&mut out);
        out
    }

    /// Splits `target` in two, placing `new` right of / below it, and
    /// makes `new` active. The divider's id comes from `ids`.
    ///
    /// # Errors
    ///
    /// Returns [`OrzmuxError::UnresolvedTarget`] when `target` is not in
    /// the tree.
    pub fn split(
        &mut self,
        ids: &mut SplitIds,
        target: PaneId,
        orientation: SplitOrientation,
        new: PaneId,
    ) -> OrzmuxResult {
        if !self.root.split_leaf(target, orientation, new, ids.mint()) {
            return Err(OrzmuxError::UnresolvedTarget);
        }
        self.activate(new);
        Ok(())
    }

    /// Removes `pane`, handing its space to its sibling. A removed active
    /// pane is replaced by the most recently active survivor, or by the
    /// first pane left-to-right / top-to-bottom when no survivor was
    /// activated before. The tree's only pane is never removed.
    pub fn remove(self, pane: PaneId) -> Removal {
        let Self {
            root,
            active,
            mut history,
        } = self;
        match root.without(pane) {
            Pruned::Absent(root) => Removal::Absent(Self {
                root,
                active,
                history,
            }),
            Pruned::Only(root) => Removal::Last(Self {
                root,
                active,
                history,
            }),
            Pruned::Removed(root) => {
                history.retain(|p| *p != pane);
                let active = if active == pane {
                    history.pop().unwrap_or_else(|| root.first_leaf())
                } else {
                    active
                };
                Removal::Removed(Self {
                    root,
                    active,
                    history,
                })
            }
        }
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
        let active = self.active;
        let tiling = self.tile(window);
        let Some(from) = tiling.rect_of(active) else {
            return false;
        };
        let best = tiling
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

    /// Tiles `window` with the tree's panes and separators, using
    /// `max(window, minimum)` per axis so every pane keeps at least one
    /// cell; the caller clips what overflows.
    pub fn tile(&self, window: GridSize) -> Tiling {
        let rect = self.root_rect(window);
        let mut tiling = Tiling {
            size: GridSize {
                cols: rect.cols,
                rows: rect.rows,
            },
            panes: Vec::new(),
            separators: Vec::new(),
        };
        self.root.tile_into(&mut tiling, rect);
        tiling
    }

    /// Moves `split`'s divider to `position`, a whole-window cell
    /// boundary, clamped so neither side falls below the drag minimum.
    /// A split whose area cannot give both sides that minimum falls back
    /// to the tree's own minimum. Returns whether the ratio changed; a
    /// `split` that is not in the tree returns `false`.
    pub fn resize_split(&mut self, split: SplitId, position: u16, window: GridSize) -> bool {
        let Some((node, rect)) = self.split_mut(split, window) else {
            return false;
        };
        let range = DividerRange::of(node, rect);
        let first = position
            .saturating_sub(range.origin)
            .clamp(range.lo, range.hi);
        node.set_first_len(first, range.avail)
    }

    /// Moves one divider of the active pane `cells` cells in `direction`.
    ///
    /// The divider comes from the pane's run on `direction`'s axis: the
    /// nearest split of that orientation above the pane, together with
    /// each consecutive parent of the same orientation. The run stops at
    /// the first split of the other orientation. Within the run the
    /// divider after the pane moves when there is one, and otherwise the
    /// divider before it.
    ///
    /// The divider stops at the drag minimum, or at the tree minimum when
    /// its split's area cannot give both sides the drag minimum, and never
    /// moves against `direction`. Returns whether the ratio changed; it is
    /// `false` with no active pane, no divider on the axis, a zero `cells`,
    /// or no room left to move.
    pub fn resize_direction(
        &mut self,
        direction: PaneDirection,
        cells: u16,
        window: GridSize,
    ) -> bool {
        let active = self.active;
        let (orientation, toward_first) = match direction {
            PaneDirection::Left => (SplitOrientation::Vertical, true),
            PaneDirection::Right => (SplitOrientation::Vertical, false),
            PaneDirection::Up => (SplitOrientation::Horizontal, true),
            PaneDirection::Down => (SplitOrientation::Horizontal, false),
        };
        let Some(split) = self.border_split(active, orientation) else {
            return false;
        };
        let Some((node, rect)) = self.split_mut(split, window) else {
            return false;
        };
        let range = DividerRange::of(node, rect);
        let current = node.first_len(range.avail);
        let (next, moves) = if toward_first {
            let next = current.saturating_sub(cells).max(range.lo);
            (next, next < current)
        } else {
            let next = current.saturating_add(cells).min(range.hi);
            (next, next > current)
        };
        moves && node.set_first_len(next, range.avail)
    }

    /// Whether `pane` is a leaf of the tree.
    pub fn contains(&self, pane: PaneId) -> bool {
        self.root.has_leaf(pane)
    }

    #[cfg(test)]
    fn set_root_ratio_for_test(&mut self, ratio: f32) {
        if let Node::Split(split) = &mut self.root {
            split.ratio = ratio;
        }
    }

    /// Forgets every activation but the active pane's own.
    #[cfg(test)]
    fn clear_history_for_test(&mut self) {
        self.history.clear();
    }

    #[cfg(test)]
    fn min_size_for_drag(&self) -> GridSize {
        self.root.min_size_for_drag()
    }

    fn activate(&mut self, pane: PaneId) {
        if pane == self.active {
            return;
        }
        self.history.retain(|p| *p != pane);
        self.history.push(self.active);
        self.active = pane;
    }

    /// Position in the activation history (higher is more recent, the
    /// active pane highest), or `None` for a pane never activated.
    fn recency(&self, pane: PaneId) -> Option<usize> {
        if pane == self.active {
            return Some(self.history.len());
        }
        self.history.iter().position(|p| *p == pane)
    }

    /// The rectangle the tree tiles for `window`: the window widened per
    /// axis to the tree's minimum.
    fn root_rect(&self, window: GridSize) -> Rect {
        let min = self.root.min_size();
        Rect {
            x: 0,
            y: 0,
            cols: window.cols.max(min.cols),
            rows: window.rows.max(min.rows),
        }
    }

    /// The split with `id` and the rectangle it divides when the tree
    /// tiles `window`, or `None` when it is not in the tree.
    fn split_mut(&mut self, id: SplitId, window: GridSize) -> Option<(&mut Split, Rect)> {
        let root_rect = self.root_rect(window);
        self.root.find_split_mut(id, root_rect)
    }

    /// The split whose divider a directional resize of `pane` on
    /// `orientation`'s axis moves, or `None` when no split of that
    /// orientation sits above `pane`.
    fn border_split(&self, pane: PaneId, orientation: SplitOrientation) -> Option<SplitId> {
        let path = self.root.path_to(pane)?;
        let mut run = path
            .iter()
            .skip_while(|a| a.orientation != orientation)
            .take_while(|a| a.orientation == orientation)
            .peekable();
        let nearest = *run.peek()?;
        Some(run.find(|a| a.in_first).unwrap_or(nearest).split)
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

    fn tile_into(&self, out: &mut Tiling, rect: Rect) {
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
                s.first.tile_into(out, first_rect);
                out.separators.push(separator);
                s.second.tile_into(out, second_rect);
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

    /// The pane left-to-right / top-to-bottom first in this subtree.
    fn first_leaf(&self) -> PaneId {
        match self {
            Node::Leaf(id) => *id,
            Node::Split(s) => s.first.first_leaf(),
        }
    }

    /// The subtree without `pane`: the split immediately containing the
    /// removed leaf collapses into its sibling, while an ancestor split
    /// whose child only shrank keeps its structure around that child. A
    /// subtree that is `pane`'s own leaf comes back as [`Pruned::Only`].
    fn without(self, pane: PaneId) -> Pruned {
        match self {
            Node::Leaf(id) if id == pane => Pruned::Only(Node::Leaf(id)),
            Node::Leaf(id) => Pruned::Absent(Node::Leaf(id)),
            Node::Split(Split {
                id,
                orientation,
                ratio,
                first,
                second,
            }) => {
                let rebuild = |first: Box<Node>, second: Box<Node>| {
                    Node::Split(Split {
                        id,
                        orientation,
                        ratio,
                        first,
                        second,
                    })
                };
                match (*first).without(pane) {
                    Pruned::Only(_) => Pruned::Removed(*second),
                    Pruned::Removed(first) => Pruned::Removed(rebuild(Box::new(first), second)),
                    Pruned::Absent(first) => match (*second).without(pane) {
                        Pruned::Only(_) => Pruned::Removed(first),
                        Pruned::Removed(second) => {
                            Pruned::Removed(rebuild(Box::new(first), Box::new(second)))
                        }
                        Pruned::Absent(second) => {
                            Pruned::Absent(rebuild(Box::new(first), Box::new(second)))
                        }
                    },
                }
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

    /// The splits from `pane` up to this node, nearest first, or `None`
    /// when `pane` is not in this subtree.
    fn path_to(&self, pane: PaneId) -> Option<Vec<Ancestor>> {
        match self {
            Node::Leaf(id) => (*id == pane).then(Vec::new),
            Node::Split(s) => {
                [(&s.first, true), (&s.second, false)]
                    .into_iter()
                    .find_map(|(child, in_first)| {
                        let mut path = child.path_to(pane)?;
                        path.push(Ancestor {
                            split: s.id,
                            orientation: s.orientation,
                            in_first,
                        });
                        Some(path)
                    })
            }
        }
    }
}

impl Split {
    /// The first child's extent along the split axis when the children
    /// share `avail` cells, clamped so both keep their tree minimum.
    fn first_len(&self, avail: u16) -> u16 {
        share(
            avail,
            self.ratio,
            self.along(self.first.min_size()),
            self.along(self.second.min_size()),
        )
    }

    /// Points the ratio at `first` of `avail` cells. Returns whether the
    /// ratio changed.
    fn set_first_len(&mut self, first: u16, avail: u16) -> bool {
        let ratio = f32::from(first) / f32::from(avail);
        if self.ratio == ratio {
            return false;
        }
        self.ratio = ratio;
        true
    }

    /// The child rectangles `rect` divides into, and the divider between
    /// them.
    fn subdivide(&self, rect: Rect) -> (Rect, Separator, Rect) {
        match self.orientation {
            SplitOrientation::Vertical => {
                let avail = rect.cols - 1;
                let first = self.first_len(avail);
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
                let first = self.first_len(avail);
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

    /// The extent of `size` along the split axis.
    fn along(&self, size: GridSize) -> u16 {
        match self.orientation {
            SplitOrientation::Vertical => size.cols,
            SplitOrientation::Horizontal => size.rows,
        }
    }
}

impl DividerRange {
    /// The divider range `split` allows within `rect`: both children keep
    /// the drag minimum, or the tree minimum when `rect` is too small for
    /// the drag minimum.
    fn of(split: &Split, rect: Rect) -> Self {
        let (extent, origin) = match split.orientation {
            SplitOrientation::Vertical => (rect.cols, rect.x),
            SplitOrientation::Horizontal => (rect.rows, rect.y),
        };
        let avail = extent - 1;
        let drag_lo = split.along(split.first.min_size_for_drag());
        let drag_hi = avail.saturating_sub(split.along(split.second.min_size_for_drag()));
        let (lo, hi) = if drag_lo > drag_hi {
            (
                split.along(split.first.min_size()),
                avail - split.along(split.second.min_size()),
            )
        } else {
            (drag_lo, drag_hi)
        };
        Self {
            avail,
            origin,
            lo,
            hi,
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
mod tests;
