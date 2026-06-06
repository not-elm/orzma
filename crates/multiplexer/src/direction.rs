//! Direction enums for pane focus movement and cyclic traversal.

/// Cardinal direction for pane-focus movement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneDirection {
    /// Move focus toward the top of the workspace.
    Up,
    /// Move focus toward the bottom of the workspace.
    Down,
    /// Move focus toward the left of the workspace.
    Left,
    /// Move focus toward the right of the workspace.
    Right,
}

/// Cycle direction for operations like `swap_pane` that traverse the
/// depth-first leaf order.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum CycleDirection {
    /// Move to the next pane in DFS order (wraps at end).
    Next,
    /// Move to the previous pane in DFS order (wraps at start).
    Prev,
}
