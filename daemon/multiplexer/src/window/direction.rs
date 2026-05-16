//! Direction-resolution algorithm for `Window::pane_in_direction`. Owns the
//! `PaneDirection` enum and pure adjacency / overlap helpers. No I/O.

use serde::{Deserialize, Serialize};

/// Cardinal direction for pane-focus movement. Distinct from
/// `ozmux_configs::Direction` (UX layer) to keep the multiplexer crate free
/// of a `configs` dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PaneDirection {
    /// Move focus toward the top of the window.
    Up,
    /// Move focus toward the bottom of the window.
    Down,
    /// Move focus toward the left of the window.
    Left,
    /// Move focus toward the right of the window.
    Right,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_direction_serializes_kebab_case() {
        let json = serde_json::to_string(&PaneDirection::Up).unwrap();
        assert_eq!(json, "\"up\"");
        let back: PaneDirection = serde_json::from_str(&json).unwrap();
        assert_eq!(back, PaneDirection::Up);
    }
}
