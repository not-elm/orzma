//! App-owned focus ring across native ratatui widgets and embedded webviews,
//! plus the glue signal channel and the spatial-navigation resolver.

use crate::error::OzmaResult;
use crate::session::Ozma;
use crate::webview::WebviewHandle;
use crossbeam_channel::{Receiver, Sender, unbounded};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;

/// A spatial navigation direction (vim `h/j/k/l`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Left (`h`).
    Left,
    /// Down (`j`).
    Down,
    /// Up (`k`).
    Up,
    /// Right (`l`).
    Right,
}

/// What the host must be told after a focus-ring change.
#[derive(Debug, Clone, PartialEq)]
pub enum FocusSync {
    /// Focus did not move; tell the host nothing.
    Unchanged,
    /// Focus moved onto a webview; the app should call `handle.focus()`.
    Focus(WebviewHandle),
    /// Focus moved onto a native widget; the app should call `ozma.blur()`.
    Blur,
}

impl FocusSync {
    /// Applies the sync by sending the matching control-plane op.
    pub fn apply(&self, ozma: &Ozma) -> OzmaResult<()> {
        match self {
            FocusSync::Unchanged => Ok(()),
            FocusSync::Focus(handle) => handle.focus(),
            FocusSync::Blur => ozma.blur(),
        }
    }
}

/// A glue signal delivered from a webview page over the reserved `__ozma.*` RPC.
#[derive(Debug, Clone, PartialEq)]
enum Signal {
    /// The page requested a directional focus move (handle, direction).
    Nav(String, Direction),
    /// The page reported a DOM focus change (handle, focused?).
    Focus(String, bool),
}

#[derive(Debug, Clone, PartialEq)]
enum ItemKind {
    Native,
    Webview(WebviewHandle),
}

#[derive(Debug, Clone)]
struct Item {
    id: String,
    kind: ItemKind,
    rect: Option<Rect>,
}

/// An app-owned focus ring across native widgets and webviews.
///
/// The app registers focusable items, feeds it nav keys (when a native widget
/// is focused) and drained glue signals (when a webview is focused), and renders
/// using [`FocusManager::is_focused`]. Each transition yields a [`FocusSync`] the
/// app applies to keep the host's `FocusedWebview` in step.
pub struct FocusManager {
    items: Vec<Item>,
    focused: Option<usize>,
    tx: Sender<Signal>,
    rx: Receiver<Signal>,
}

impl FocusManager {
    /// Creates an empty focus ring.
    pub fn new() -> Self {
        let (tx, rx) = unbounded();
        Self {
            items: Vec::new(),
            focused: None,
            tx,
            rx,
        }
    }

    /// Maps a reserved nav chord to a [`Direction`] (default `Alt+h/j/k/l`).
    pub fn nav_key(key: &KeyEvent) -> Option<Direction> {
        if !key.modifiers.contains(KeyModifiers::ALT) {
            return None;
        }
        match key.code {
            KeyCode::Char('h') => Some(Direction::Left),
            KeyCode::Char('j') => Some(Direction::Down),
            KeyCode::Char('k') => Some(Direction::Up),
            KeyCode::Char('l') => Some(Direction::Right),
            _ => None,
        }
    }
}

impl Default for FocusManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Orthogonal-displacement weight in the directional cost (smart-TV LRUD tuning).
const ORTHOGONAL_WEIGHT: f32 = 0.4;

/// Resolves the nearest focus candidate in the pushed `dir` from `from`.
///
/// Filters to candidates strictly beyond `from`'s far edge in `dir` (half-plane),
/// then minimizes `primary_gap + ORTHOGONAL_WEIGHT * orthogonal_displacement`.
/// Ties resolve to the lowest candidate index for determinism. Returns the
/// winning candidate's index, or `None` when the half-plane is empty.
fn resolve_spatial(candidates: &[(usize, Rect)], from: Rect, dir: Direction) -> Option<usize> {
    let f = Edges::of(from);
    let mut sorted: Vec<(usize, Rect)> = candidates.to_vec();
    sorted.sort_by_key(|(i, _)| *i);
    let mut best: Option<(usize, f32)> = None;
    for (idx, rect) in &sorted {
        let c = Edges::of(*rect);
        let cost = match dir {
            Direction::Right if c.left >= f.right => {
                (c.left - f.right) + ORTHOGONAL_WEIGHT * (c.cy - f.cy).abs()
            }
            Direction::Left if c.right <= f.left => {
                (f.left - c.right) + ORTHOGONAL_WEIGHT * (c.cy - f.cy).abs()
            }
            Direction::Down if c.top >= f.bottom => {
                (c.top - f.bottom) + ORTHOGONAL_WEIGHT * (c.cx - f.cx).abs()
            }
            Direction::Up if c.bottom <= f.top => {
                (f.top - c.bottom) + ORTHOGONAL_WEIGHT * (c.cx - f.cx).abs()
            }
            _ => continue,
        };
        match best {
            Some((_, best_cost)) if cost >= best_cost => {}
            _ => best = Some((*idx, cost)),
        }
    }
    best.map(|(idx, _)| idx)
}

/// Edge coordinates of a rect as floats (centers included), for cost math.
struct Edges {
    left: f32,
    right: f32,
    top: f32,
    bottom: f32,
    cx: f32,
    cy: f32,
}

impl Edges {
    fn of(r: Rect) -> Self {
        let left = r.x as f32;
        let top = r.y as f32;
        let right = left + r.width as f32;
        let bottom = top + r.height as f32;
        Self {
            left,
            right,
            top,
            bottom,
            cx: left + r.width as f32 / 2.0,
            cy: top + r.height as f32 / 2.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(c: char, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), mods)
    }

    fn rect(x: u16, y: u16, w: u16, h: u16) -> Rect {
        Rect { x, y, width: w, height: h }
    }

    #[test]
    fn nav_key_maps_alt_hjkl() {
        assert_eq!(FocusManager::nav_key(&key('h', KeyModifiers::ALT)), Some(Direction::Left));
        assert_eq!(FocusManager::nav_key(&key('j', KeyModifiers::ALT)), Some(Direction::Down));
        assert_eq!(FocusManager::nav_key(&key('k', KeyModifiers::ALT)), Some(Direction::Up));
        assert_eq!(FocusManager::nav_key(&key('l', KeyModifiers::ALT)), Some(Direction::Right));
    }

    #[test]
    fn nav_key_ignores_bare_hjkl() {
        assert_eq!(FocusManager::nav_key(&key('h', KeyModifiers::NONE)), None);
        assert_eq!(FocusManager::nav_key(&key('x', KeyModifiers::ALT)), None);
    }

    #[test]
    fn resolve_picks_neighbor_in_pushed_direction() {
        let from = rect(0, 0, 10, 5);
        let right = rect(12, 0, 10, 5);
        let down = rect(0, 6, 10, 5);
        let cands = vec![(1usize, right), (2usize, down)];
        assert_eq!(resolve_spatial(&cands, from, Direction::Right), Some(1));
        assert_eq!(resolve_spatial(&cands, from, Direction::Down), Some(2));
    }

    #[test]
    fn resolve_filters_out_half_plane_and_breaks_ties_by_index() {
        let from = rect(10, 10, 10, 5);
        let a = rect(22, 10, 4, 5);
        let b = rect(22, 10, 4, 5);
        let cands = vec![(5usize, a), (3usize, b)];
        assert_eq!(resolve_spatial(&cands, from, Direction::Right), Some(3));
        assert_eq!(resolve_spatial(&cands, from, Direction::Left), None);
    }
}
