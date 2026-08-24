//! `RequestTtyMouseInput` and the observer that forwards it to the
//! target terminal's PTY.

use crate::OrzmaTtyHandle;
use bevy::prelude::*;
use orzma_tty::prelude::MouseReport;

/// Fired by the host UI to forward one mouse-protocol report to a specific
/// terminal entity.
#[derive(EntityEvent, Debug, Clone)]
pub struct RequestTtyMouseInput {
    #[event_target]
    pub terminal: Entity,
    /// The report, encoded at apply time against the terminal's active
    /// mouse encoding.
    pub mouse: MouseReport,
}

/// Registers the [`RequestTtyMouseInput`] apply observer.
pub(super) struct MouseInputPlugin;

impl Plugin for MouseInputPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_mouse_input);
    }
}

fn apply_mouse_input(e: On<RequestTtyMouseInput>, mut terms: Query<&mut OrzmaTtyHandle>) {
    if let Ok(mut tty) = terms.get_mut(e.terminal)
        && let Err(err) = tty.send_mouse(e.mouse)
    {
        error!(%err);
    }
}
