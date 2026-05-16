//! WebSocket wire enums (msgpack-tagged). Mirror these in TypeScript at
//! `daemon/frontend/src/browser/protocol/wire.ts` (Phase 5).
//!
//! Each enum is `#[serde(tag = "kind", rename_all = "snake_case")]` so the
//! wire form is `{ "kind": "screencast", "jpeg": <bytes>, "width": 1280,
//! "height": 800 }` and parses cleanly with msgpackr on the frontend.

use bytes::Bytes;
use serde::{Deserialize, Serialize};

/// Server-to-client message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BrowserServerMsg {
    /// One JPEG screencast frame.
    Screencast {
        /// Raw JPEG bytes.
        #[serde(with = "crate::bytes_serde")]
        jpeg: Bytes,
        /// Frame width in device pixels.
        width: u32,
        /// Frame height in device pixels.
        height: u32,
    },
    /// Latest navigation state.
    Nav {
        /// Current URL.
        url: String,
        /// Current title.
        title: String,
    },
    /// Result of a prior `CopyRequest` — the page's current selection text.
    ClipboardWrite {
        /// Selected text (`getSelection().toString()`).
        text: String,
    },
    /// A non-fatal page error (e.g. renderer crash, page navigation error).
    PageError {
        /// Human-readable error description.
        message: String,
    },
}

/// Client-to-server message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BrowserClientMsg {
    /// Mouse button or movement.
    Mouse {
        /// Press / release / move.
        mouse_kind: MouseKind,
        /// X coordinate in Chromium viewport pixels.
        x: f64,
        /// Y coordinate in Chromium viewport pixels.
        y: f64,
        /// Which button was involved.
        button: MouseButton,
        /// Modifier bitmask (Alt=1, Ctrl=2, Meta=4, Shift=8 — matches CDP).
        modifiers: u32,
    },
    /// Mouse wheel.
    Wheel {
        /// X coordinate.
        x: f64,
        /// Y coordinate.
        y: f64,
        /// Horizontal delta.
        dx: f64,
        /// Vertical delta.
        dy: f64,
        /// Modifier bitmask.
        modifiers: u32,
    },
    /// Keyboard key press or release.
    Key {
        /// Press / release.
        key_kind: KeyKind,
        /// `KeyboardEvent.code` (e.g. `"KeyA"`).
        code: String,
        /// `KeyboardEvent.key` (e.g. `"a"`).
        key: String,
        /// `KeyboardEvent.key` for character-producing presses; `None`
        /// for non-printable keys.
        text: Option<String>,
        /// Modifier bitmask.
        modifiers: u32,
    },
    /// IME composition update (preedit).
    ImeComposition {
        /// Current composition text.
        text: String,
        /// Selection start within `text`.
        selection_start: u32,
        /// Selection end within `text`.
        selection_end: u32,
    },
    /// IME composition commit (final).
    ImeCommit {
        /// Committed text.
        text: String,
    },
    /// Navigation command.
    Nav {
        /// The actual navigation command.
        nav: NavCommand,
    },
    /// Pane resize. Device-scale-factor is fixed at 1 in MVP per the spec.
    Resize {
        /// New viewport width in CSS pixels.
        width: u32,
        /// New viewport height in CSS pixels.
        height: u32,
    },
    /// Paste OS-clipboard text into the page.
    Paste {
        /// Text to insert.
        text: String,
    },
    /// Ask the daemon to read the page's current selection and reply with
    /// `ClipboardWrite`.
    CopyRequest,
}

/// Mouse button kind, mirrored to CDP `MouseButton`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseKind {
    /// Press.
    Down,
    /// Release.
    Up,
    /// Move.
    Move,
}

/// Which mouse button is involved in the event.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    /// Primary button.
    Left,
    /// Wheel button.
    Middle,
    /// Secondary button.
    Right,
    /// No button (e.g. movement-only events).
    None,
}

/// Keyboard event kind.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyKind {
    /// Key pressed.
    Down,
    /// Key released.
    Up,
}

/// Navigation command sent by the client.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NavCommand {
    /// Load a URL.
    Navigate {
        /// Target URL.
        url: String,
    },
    /// Go back one history entry.
    Back,
    /// Go forward one history entry.
    Forward,
    /// Reload the current page.
    Reload,
    /// Stop the current navigation.
    Stop,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_screencast_round_trips_msgpack() {
        let msg = BrowserServerMsg::Screencast {
            jpeg: bytes::Bytes::from_static(&[0xFF, 0xD8, 0xFF, 0xD9]),
            width: 1280,
            height: 800,
        };
        let bytes = rmp_serde::to_vec_named(&msg).unwrap();
        let back: BrowserServerMsg = rmp_serde::from_slice(&bytes).unwrap();
        match back {
            BrowserServerMsg::Screencast { width, height, .. } => {
                assert_eq!((width, height), (1280, 800));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn server_nav_round_trips() {
        let msg = BrowserServerMsg::Nav {
            url: "https://x.com".into(),
            title: "X".into(),
        };
        let bytes = rmp_serde::to_vec_named(&msg).unwrap();
        let back: BrowserServerMsg = rmp_serde::from_slice(&bytes).unwrap();
        assert!(matches!(back, BrowserServerMsg::Nav { ref url, .. } if url == "https://x.com"));
    }

    #[test]
    fn client_nav_navigate_round_trips() {
        let msg = BrowserClientMsg::Nav {
            nav: NavCommand::Navigate {
                url: "https://example.com".into(),
            },
        };
        let bytes = rmp_serde::to_vec_named(&msg).unwrap();
        let back: BrowserClientMsg = rmp_serde::from_slice(&bytes).unwrap();
        assert!(
            matches!(back, BrowserClientMsg::Nav { nav: NavCommand::Navigate { ref url } } if url == "https://example.com")
        );
    }

    #[test]
    fn client_mouse_round_trips() {
        let msg = BrowserClientMsg::Mouse {
            mouse_kind: MouseKind::Down,
            x: 100.0,
            y: 200.0,
            button: MouseButton::Left,
            modifiers: 4,
        };
        let bytes = rmp_serde::to_vec_named(&msg).unwrap();
        let back: BrowserClientMsg = rmp_serde::from_slice(&bytes).unwrap();
        match back {
            BrowserClientMsg::Mouse {
                x, y, modifiers, ..
            } => {
                assert_eq!((x as i64, y as i64, modifiers), (100, 200, 4));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn client_copy_request_round_trips_as_unit_variant() {
        let msg = BrowserClientMsg::CopyRequest;
        let bytes = rmp_serde::to_vec_named(&msg).unwrap();
        let back: BrowserClientMsg = rmp_serde::from_slice(&bytes).unwrap();
        assert!(matches!(back, BrowserClientMsg::CopyRequest));
    }
}
