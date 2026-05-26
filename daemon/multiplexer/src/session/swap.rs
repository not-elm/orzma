//! Pane swap types: traversal offset and mutation outcome.

use crate::session::pane::PaneId;
use serde::{Deserialize, Serialize};

/// Selects the swap target relative to the active pane in the depth-first
/// leaf traversal of the session's cell tree. Mirrors
/// `ozmux_configs::SwapOffset` so the multiplexer crate stays free of the
/// `configs` dependency (same pattern as `PaneDirection`).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SwapOffset {
    /// Swap with the previous pane (wraps around at the start).
    Prev,
    /// Swap with the next pane (wraps around at the end).
    Next,
}

/// Result of `Session::swap_pane`: whether a swap actually happened, and if
/// so, the id of the pane that traded places.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SwapOutcome {
    /// A swap was applied. `other_pane` is the id whose cell now hosts the
    /// caller's `pane` argument's previous cell.
    Swapped { other_pane: PaneId },
    /// Single-pane session — no swap target exists. Callers should treat as
    /// a soft no-op (HTTP 204, no broadcast).
    NoOp,
}
