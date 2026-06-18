//! Window-fill resize system for the Ozma terminal.

use crate::spawn::OzmaTerminal;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use ozma_tty_engine::{Coalescer, PtyHandle, TerminalHandle};
use ozma_tty_renderer::TerminalCellMetricsResource;

/// Tracks the last (cols, rows) sent to the terminal to guard against
/// redundant resize calls.
#[derive(Resource, Default)]
pub(crate) struct OzmaLastSize(pub(crate) Option<(u16, u16)>);

/// Resizes the Ozma terminal to fill the primary window.
pub(crate) fn resize_to_window(
    mut _commands: Commands,
    mut _last_size: ResMut<OzmaLastSize>,
    mut _terminal_q: Query<
        (Entity, &mut TerminalHandle, &mut PtyHandle, &mut Coalescer),
        With<OzmaTerminal>,
    >,
    _metrics: Option<Res<TerminalCellMetricsResource>>,
    _window_q: Query<&Window, With<PrimaryWindow>>,
) {
    // TODO: implement in Task 5
}
