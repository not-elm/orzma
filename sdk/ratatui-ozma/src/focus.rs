//! App-owned focus ring across native ratatui widgets and embedded webviews,
//! plus the glue signal channel and the spatial-navigation resolver.

use crate::error::OzmaResult;
use crate::session::Ozma;
use crate::webview::{Webview, WebviewHandle};
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

/// An opaque glue signal forwarded from a webview page to a [`FocusManager`].
///
/// Delivered over the reserved `__ozma.*` RPC handlers installed by [`focusable`].
#[derive(Debug, Clone, PartialEq)]
pub enum Signal {
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

    /// Registers a native widget without geometry (Tab-style only).
    pub fn add_native(&mut self, id: impl Into<String>) {
        self.push(Item {
            id: id.into(),
            kind: ItemKind::Native,
            rect: None,
        });
    }

    /// Registers a native widget with its current layout rect (spatial nav).
    pub fn add_native_at(&mut self, id: impl Into<String>, rect: Rect) {
        self.push(Item {
            id: id.into(),
            kind: ItemKind::Native,
            rect: Some(rect),
        });
    }

    /// Registers a webview widget with its current layout rect.
    pub fn add_webview_at(&mut self, id: impl Into<String>, handle: WebviewHandle, rect: Rect) {
        self.push(Item {
            id: id.into(),
            kind: ItemKind::Webview(handle),
            rect: Some(rect),
        });
    }

    /// Updates the recorded rect of a registered item (call each frame).
    pub fn set_rect(&mut self, id: &str, rect: Rect) {
        if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
            item.rect = Some(rect);
        }
    }

    /// Returns a sender for reserved-glue signals; clone into webview handlers.
    pub fn signal_sender(&self) -> Sender<Signal> {
        self.tx.clone()
    }

    /// Whether `id` is the currently-focused item.
    pub fn is_focused(&self, id: &str) -> bool {
        self.focused.is_some_and(|i| self.items[i].id == id)
    }

    /// Whether the focused item is a native widget (or nothing is focused).
    pub fn focused_is_native(&self) -> bool {
        match self.focused {
            Some(i) => matches!(self.items[i].kind, ItemKind::Native),
            None => true,
        }
    }

    /// Moves focus spatially in `dir`, returning the host sync to apply.
    pub fn navigate(&mut self, dir: Direction) -> FocusSync {
        let Some(from_idx) = self.focused else {
            return FocusSync::Unchanged;
        };
        let Some(from) = self.items[from_idx].rect else {
            return FocusSync::Unchanged;
        };
        let candidates: Vec<(usize, Rect)> = self
            .items
            .iter()
            .enumerate()
            .filter(|(i, item)| *i != from_idx && item.rect.is_some())
            .map(|(i, item)| (i, item.rect.unwrap()))
            .collect();
        match resolve_spatial(&candidates, from, dir) {
            Some(next) => self.focus_index(next),
            None => FocusSync::Unchanged,
        }
    }

    /// Drains queued glue signals, applying each to the ring; returns the syncs
    /// (in order) the app must apply to the host.
    pub fn drain(&mut self) -> Vec<FocusSync> {
        let mut out = Vec::new();
        while let Ok(signal) = self.rx.try_recv() {
            let sync = match signal {
                Signal::Nav(handle, dir) => {
                    if self.is_focused_handle(&handle) {
                        self.navigate(dir)
                    } else {
                        FocusSync::Unchanged
                    }
                }
                Signal::Focus(handle, true) => match self.index_of_handle(&handle) {
                    Some(idx) => self.focus_index(idx),
                    None => FocusSync::Unchanged,
                },
                Signal::Focus(handle, false) => {
                    if self.is_focused_handle(&handle) {
                        self.focus_first_native()
                    } else {
                        FocusSync::Unchanged
                    }
                }
            };
            if !matches!(sync, FocusSync::Unchanged) {
                out.push(sync);
            }
        }
        out
    }

    fn push(&mut self, item: Item) {
        self.items.push(item);
        if self.focused.is_none() {
            self.focused = Some(self.items.len() - 1);
        }
    }

    fn focus_index(&mut self, idx: usize) -> FocusSync {
        if self.focused == Some(idx) {
            return FocusSync::Unchanged;
        }
        self.focused = Some(idx);
        match &self.items[idx].kind {
            ItemKind::Webview(handle) => FocusSync::Focus(handle.clone()),
            ItemKind::Native => FocusSync::Blur,
        }
    }

    fn focus_first_native(&mut self) -> FocusSync {
        match self
            .items
            .iter()
            .position(|i| matches!(i.kind, ItemKind::Native))
        {
            Some(idx) => self.focus_index(idx),
            None => FocusSync::Unchanged,
        }
    }

    fn index_of_handle(&self, handle: &str) -> Option<usize> {
        self.items.iter().position(|i| match &i.kind {
            ItemKind::Webview(h) => h.id() == handle,
            ItemKind::Native => false,
        })
    }

    fn is_focused_handle(&self, handle: &str) -> bool {
        self.focused
            .map(|i| match &self.items[i].kind {
                ItemKind::Webview(h) => h.id() == handle,
                ItemKind::Native => false,
            })
            .unwrap_or(false)
    }

    #[cfg(test)]
    pub(crate) fn rx_for_test(&self) -> &Receiver<Signal> {
        &self.rx
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

/// Instruments a [`Webview`] with the reserved `__ozma.nav` / `__ozma.focus`
/// handlers that forward page glue signals into a [`FocusManager`].
///
/// Pass `fm.signal_sender()`. Call before `Ozma::register`. The page's
/// `location.hostname` (its minted handle) is the first RPC arg, so signals
/// route to the right ring item even with multiple webviews.
pub fn focusable(view: Webview, tx: Sender<Signal>) -> Webview {
    let nav_tx = tx.clone();
    let view = view.on_reserved("__ozma.nav", move |(handle, dir): (String, String)| {
        let direction = match dir.as_str() {
            "left" => Direction::Left,
            "down" => Direction::Down,
            "up" => Direction::Up,
            "right" => Direction::Right,
            other => return Err(crate::error::RpcError::new(format!("bad dir: {other}"))),
        };
        let _ = nav_tx.send(Signal::Nav(handle, direction));
        Ok::<_, crate::error::RpcError>(())
    });
    view.on_reserved("__ozma.focus", move |(handle, focused): (String, bool)| {
        let _ = tx.send(Signal::Focus(handle, focused));
        Ok::<_, crate::error::RpcError>(())
    })
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

    fn pair_handle(id: &str) -> WebviewHandle {
        use std::os::unix::net::UnixStream;
        use std::sync::{Arc, Mutex};
        let (a, _b) = UnixStream::pair().unwrap();
        WebviewHandle::new(id.to_owned(), Arc::new(Mutex::new(a)))
    }

    #[test]
    fn navigate_native_to_webview_returns_focus_sync() {
        let mut fm = FocusManager::new();
        fm.add_native_at("left", rect(0, 0, 10, 5));
        fm.add_webview_at("right", pair_handle("view-r"), rect(12, 0, 10, 5));
        assert!(fm.is_focused("left"));
        assert!(fm.focused_is_native());
        let sync = fm.navigate(Direction::Right);
        assert!(matches!(sync, FocusSync::Focus(_)));
        assert!(fm.is_focused("right"));
        assert!(!fm.focused_is_native());
    }

    #[test]
    fn navigate_webview_to_native_returns_blur() {
        let mut fm = FocusManager::new();
        fm.add_webview_at("left", pair_handle("view-l"), rect(0, 0, 10, 5));
        fm.add_native_at("right", rect(12, 0, 10, 5));
        let _ = fm.navigate(Direction::Right);
        let sync = fm.navigate(Direction::Left);
        assert!(matches!(sync, FocusSync::Focus(_)));
        let blur = fm.navigate(Direction::Right);
        assert_eq!(blur, FocusSync::Blur);
    }

    #[test]
    fn navigate_no_neighbor_is_unchanged() {
        let mut fm = FocusManager::new();
        fm.add_native_at("only", rect(0, 0, 10, 5));
        assert_eq!(fm.navigate(Direction::Right), FocusSync::Unchanged);
    }

    #[test]
    fn drain_applies_nav_signal_from_focused_webview() {
        let mut fm = FocusManager::new();
        fm.add_webview_at("wv", pair_handle("view-x"), rect(0, 0, 10, 5));
        fm.add_native_at("native", rect(12, 0, 10, 5));
        fm.signal_sender()
            .send(Signal::Nav("view-x".into(), Direction::Right))
            .unwrap();
        let syncs = fm.drain();
        assert_eq!(syncs, vec![FocusSync::Blur]);
        assert!(fm.is_focused("native"));
    }

    #[test]
    fn drain_focus_report_reconciles_click() {
        let mut fm = FocusManager::new();
        fm.add_native_at("native", rect(0, 0, 10, 5));
        fm.add_webview_at("wv", pair_handle("view-x"), rect(12, 0, 10, 5));
        assert!(fm.is_focused("native"));
        fm.signal_sender()
            .send(Signal::Focus("view-x".into(), true))
            .unwrap();
        let _ = fm.drain();
        assert!(fm.is_focused("wv"));
    }

    #[test]
    fn focusable_installs_reserved_handlers_that_feed_the_channel() {
        let fm = FocusManager::new();
        let view = focusable(Webview::inline("x"), fm.signal_sender());
        let nav = view.handlers_for_test().get("__ozma.nav").expect("nav handler");
        nav(vec![serde_json::json!("view-x"), serde_json::json!("right")]).unwrap();
        match fm.rx_for_test().try_recv().unwrap() {
            Signal::Nav(h, d) => {
                assert_eq!(h, "view-x");
                assert_eq!(d, Direction::Right);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }
}
