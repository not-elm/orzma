//! GUI-side multiplexer helpers: action dispatcher and layout-change
//! logging. The core ECS-native domain model lives in the
//! `ozmux_multiplexer` crate and is imported directly by consumers.

use bevy::prelude::*;

pub mod commands;
pub mod log;

/// Monotonic counter for auto-naming sessions created by the GUI
/// (bootstrap + `Action::NewSession` via CMD+R). Each call to `next`
/// returns the next `"session{n}"` string (1-based, never reused even
/// after a session is closed).
#[derive(Resource, Default, Debug)]
pub(crate) struct SessionNameCounter(u32);

impl SessionNameCounter {
    /// Mint the next auto-generated session name and increment the
    /// counter in lockstep. Saturating addition prevents an extremely
    /// unlikely u32 overflow from panicking; the resulting collision
    /// would be cosmetic and only after ~4 billion sessions.
    pub(crate) fn next(&mut self) -> String {
        self.0 = self.0.saturating_add(1);
        format!("session{}", self.0)
    }
}
