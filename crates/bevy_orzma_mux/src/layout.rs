//! The latest layout snapshot the drain received, and (from Task 9) the
//! system that applies it to pane nodes.

use bevy::prelude::*;
use orzma_mux::prelude::Layout;

/// The latest layout snapshot. Written by the drain only when it
/// differs; the non-empty → empty transition is detected there.
#[derive(Resource, Default, Debug, PartialEq)]
pub struct CurrentLayout(pub Layout);
