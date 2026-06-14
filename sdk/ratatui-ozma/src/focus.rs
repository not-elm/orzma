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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(c: char, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), mods)
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
}
