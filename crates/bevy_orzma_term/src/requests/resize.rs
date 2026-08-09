//! `RequestTermResize`: the new grid size the host UI asks a terminal
//! entity to adopt.

use bevy::prelude::*;

/// Fired by the host UI to resize a specific terminal entity's grid.
///
/// Carries the target size in cells, not pixels — the host owns the
/// cell-metrics math and hands over an already-resolved column/row count.
#[derive(EntityEvent, Debug, Clone)]
pub struct RequestTermResize {
    #[event_target]
    pub terminal: Entity,
    /// Target column count.
    pub cols: u16,
    /// Target row count.
    pub rows: u16,
}

pub(super) struct ResizePlugin;

impl Plugin for ResizePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_resize);
    }
}

fn apply_resize(e: On<RequestTermResize>) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OrzmaTermHandle;

    fn app_with_terminal() -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins(ResizePlugin);
        let (handle, _) = OrzmaTermHandle::detached(80, 24);
        let terminal = app.world_mut().spawn(handle).id();
        (app, terminal)
    }

    /// Reads the terminal's PTY grid size back from the kernel.
    fn pty_size(app: &App, terminal: Entity) -> (u16, u16) {
        let handle = app
            .world()
            .entity(terminal)
            .get::<OrzmaTermHandle>()
            .expect("terminal entity must keep its handle");
        let size = handle.pty_size();
        (size.cols, size.rows)
    }

    /// Asserts that a resize request lands on the PTY: the size read
    /// back from the kernel (`TIOCGWINSZ`) is the requested one.
    ///
    /// Case: the ordinary window-resize path — the host has resolved
    /// pixels to cells and fires one request at the terminal it owns.
    /// The child process only learns its new size through the ioctl on
    /// the master (and the SIGWINCH it raises), so a resize that stops
    /// short of the PTY leaves every TUI app drawing at the stale size
    /// while the renderer shows a larger grid.
    #[test]
    fn resize_applies_the_requested_size_to_the_pty() {
        let (mut app, terminal) = app_with_terminal();
        app.world_mut().trigger(RequestTermResize {
            terminal,
            cols: 120,
            rows: 40,
        });
        assert_eq!(pty_size(&app, terminal), (120, 40));
    }

    /// Asserts that a request with a zero axis leaves the PTY size
    /// untouched.
    ///
    /// Case: a minimized window, or a frame before the cell metrics
    /// have loaded, makes the host compute 0 columns or rows. The
    /// agreed policy is to ignore the request outright — clamping to
    /// 1x1 was rejected because shrinking the grid for a transient
    /// state risks reflow/scrollback loss on the way back. The guard
    /// belongs to `OrzmaTerm::resize`, mirroring `write_paste`'s
    /// empty-text no-op; do not "fix" this test toward clamping.
    #[test]
    fn a_degenerate_resize_is_ignored() {
        let (mut app, terminal) = app_with_terminal();
        for (cols, rows) in [(0, 0), (0, 40), (120, 0)] {
            app.world_mut().trigger(RequestTermResize {
                terminal,
                cols,
                rows,
            });
            assert_eq!(
                pty_size(&app, terminal),
                (80, 24),
                "resize {cols}x{rows} must be ignored"
            );
        }
    }
}
