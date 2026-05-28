//! GUI-side multiplexer helpers: action dispatcher and layout-change
//! logging. The core ECS-native domain model lives in the
//! `ozmux_multiplexer` crate and is imported directly by consumers.

use bevy::prelude::*;

pub mod commands;
pub mod log;

/// Monotonic counter for sessions created by the GUI (bootstrap +
/// `Action::NewSession` via CMD+R). Each call to `next` returns the
/// next index (1-based, never reused even after a session is closed).
/// The value also seeds the `"session{n}"` auto-name and the
/// `SessionCreatedAt` Component used for stable display order.
#[derive(Resource, Default, Debug)]
pub(crate) struct SessionNameCounter(u32);

impl SessionNameCounter {
    /// Mint the next creation-order index. Saturating addition prevents
    /// an extremely unlikely u32 overflow from panicking; the resulting
    /// collision would be cosmetic and only after ~4 billion sessions.
    pub(crate) fn next(&mut self) -> u32 {
        self.0 = self.0.saturating_add(1);
        self.0
    }
}

/// Per-Session monotonic creation-order index, set at spawn time from
/// `SessionNameCounter`. Used as the stable sort key for any UI that
/// lists sessions in creation order (status bar, focus cycling, ...).
/// `Entity` ordering is unreliable for this purpose because Bevy's
/// deferred command queues do not guarantee strictly monotonic indices
/// across multiple `Commands` instances.
#[derive(Component, Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct SessionCreatedAt(pub(crate) u32);
