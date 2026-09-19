//! Configuration for the caret's blink timing and drawn appearance.

use bevy::prelude::*;
use std::time::Duration;

/// The caret drawing knobs the renderer reads each frame.
#[derive(Resource, Debug, Clone, Copy)]
pub struct CaretStyle {
    /// The interval between blink phases.
    pub blink_interval: Duration,
    /// How long the caret keeps blinking with no keystroke; `None`
    /// blinks indefinitely.
    pub blink_timeout: Option<Duration>,
    /// Caret thickness as a fraction of the cell width.
    pub thickness: f32,
    /// Whether an unfocused caret is drawn as a hollow block.
    pub unfocused_hollow: bool,
}

impl Default for CaretStyle {
    fn default() -> Self {
        Self {
            blink_interval: Duration::from_millis(750),
            blink_timeout: Some(Duration::from_secs(5)),
            thickness: 0.15,
            unfocused_hollow: true,
        }
    }
}
