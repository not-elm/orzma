//! Window-fill resize system for the Ozma terminal.

use crate::spawn::OzmaTerminal;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use ozma_tty_engine::{Coalescer, PtyHandle, TerminalHandle};
use ozma_tty_renderer::TerminalCellMetricsResource;

/// Tracks the last (cols, rows) sent to the terminal to guard against
/// redundant resize calls.
#[derive(Resource, Default)]
pub(crate) struct OzmaLastSize(Option<(u16, u16)>);

/// Resets the cached terminal size on Ozma mode entry so `resize_to_window`
/// fires on the first frame even when font metrics and window size are stable.
pub(crate) fn reset_last_size(mut last_size: ResMut<OzmaLastSize>) {
    last_size.0 = None;
}

/// Resizes the Ozma terminal to fill the primary window.
///
/// Gated by `run_if` at registration — only runs when `OzmaLastSize`,
/// `TerminalCellMetricsResource`, or `WindowResized` changes.
pub(crate) fn resize_to_window(
    mut commands: Commands,
    mut last_size: ResMut<OzmaLastSize>,
    mut terminal_q: Query<
        (Entity, &mut TerminalHandle, &mut PtyHandle, &mut Coalescer),
        With<OzmaTerminal>,
    >,
    metrics: Option<Res<TerminalCellMetricsResource>>,
    window_q: Query<&Window, With<PrimaryWindow>>,
) {
    let Some(metrics) = metrics else {
        return;
    };
    let Ok(window) = window_q.single() else {
        return;
    };
    let Ok((entity, mut handle, mut pty, mut coalescer)) = terminal_q.single_mut() else {
        return;
    };

    let cell_w = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h = metrics.metrics.line_height_phys.floor().max(1.0);
    let (cols, rows) = crate::spawn::cells_for(
        window.resolution.physical_width(),
        window.resolution.physical_height(),
        cell_w,
        cell_h,
    );

    if last_size.0 == Some((cols, rows)) {
        return;
    }

    match handle.resize(&mut pty, &mut coalescer, cols, rows) {
        Ok(()) => {
            last_size.0 = Some((cols, rows));
            handle.emit_pending(&mut commands, entity);
        }
        Err(e) => tracing::warn!(?e, cols, rows, "failed to resize ozma terminal"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_size_starts_none() {
        assert!(OzmaLastSize::default().0.is_none());
    }
}
