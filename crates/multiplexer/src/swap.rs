//! Swap-pane offset enum and outcome reporting. Entity-addressed.

use bevy::ecs::entity::Entity;

/// Direction of a `swap_pane` operation in the depth-first leaf
/// traversal of the cell tree.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SwapOffset {
    /// Swap with the previous pane (wraps around at the start).
    Prev,
    /// Swap with the next pane (wraps around at the end).
    Next,
}

/// Result of `swap_pane`. `NoOp` is returned for single-pane sessions;
/// `Swapped` carries the entity of the pane on the other side of the swap.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum SwapOutcome {
    /// A swap was applied. `other_pane` is the entity whose cell now hosts
    /// the caller's `pane` argument's previous cell.
    Swapped {
        /// The other pane entity that traded places.
        other_pane: Entity,
    },
    /// Single-pane session — no swap target exists. Callers should treat as
    /// a soft no-op.
    NoOp,
}
