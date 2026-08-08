//! `RequestTermMouseInput` and the observer that forwards it to the
//! target terminal's PTY.

use crate::OrzmaTermHandle;
use bevy::prelude::*;
use orzma_term::prelude::MouseReport;

/// Fired by the host UI to forward one mouse-protocol report to a specific
/// terminal entity.
#[derive(EntityEvent, Debug, Clone)]
pub struct RequestTermMouseInput {
    #[event_target]
    pub terminal: Entity,
    /// The report, encoded at apply time against the terminal's active
    /// mouse encoding.
    pub mouse: MouseReport,
}

/// Registers the [`RequestTermMouseInput`] apply observer.
pub(super) struct MouseInputPlugin;

impl Plugin for MouseInputPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_request_term_mouse_input);
    }
}

fn apply_request_term_mouse_input(
    e: On<RequestTermMouseInput>,
    mut terms: Query<&mut OrzmaTermHandle>,
) {
    if let Ok(mut tty) = terms.get_mut(e.terminal)
        && let Err(err) = tty.write_mouse_input(e.mouse)
    {
        error!(%err);
    }
}
