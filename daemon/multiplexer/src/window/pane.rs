//! Pane module. A Pane embeds its activities and tracks the active one.

pub mod activity;

#[allow(clippy::module_inception)]
mod pane;
pub use pane::{CycleDirection, Pane, PaneId, PaneState, SetActiveOutcome};
