//! Caret paint policy: focus, hollow rendering, and the blink phase.

use bevy::prelude::*;
use std::time::Duration;

mod blink;
mod paint;
mod style;

pub use blink::blink_phase_on;
pub use paint::{CURSOR_HOLLOW_BIT, CURSOR_VISIBLE_BIT, CaretPaint, CaretPaintInput, CaretStroke};
pub use style::CaretStyle;

/// The real-time elapsed reading at the last keystroke, which the blink
/// phase counts from. A terminal that has seen no keystroke counts from
/// startup.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct LastKeyInstant(pub Duration);

/// Registers the resources the cursor paint policy reads.
pub struct CursorPlugin;

impl Plugin for CursorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CaretStyle>()
            .init_resource::<LastKeyInstant>();
    }
}
