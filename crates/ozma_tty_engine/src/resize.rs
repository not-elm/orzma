//! `TerminalResize` — resizes a terminal's PTY master to a cell grid size.

use crate::pty::PtyHandle;
use bevy::prelude::*;

/// Requests a resize of `entity`'s PTY master to `cols` x `rows` cells.
///
/// Used by the tmux adoption bridge to size the adopted gateway PTY to the GUI
/// window: tmux lays panes out to the control client's tty size, which is the
/// gateway PTY size. Pixel dimensions are left at zero (`PtyHandle::resize`
/// fills a `PtySize` with `pixel_width` / `pixel_height` of 0).
#[derive(EntityEvent)]
pub struct TerminalResize {
    /// Target terminal entity (must own a `PtyHandle`).
    #[event_target]
    pub entity: Entity,
    /// New column count for the PTY master.
    pub cols: u16,
    /// New row count for the PTY master.
    pub rows: u16,
}

/// Registers the `TerminalResize` observer.
pub struct ResizePlugin;

impl Plugin for ResizePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_resize);
    }
}

fn on_resize(ev: On<TerminalResize>, mut ptys: Query<&mut PtyHandle>) {
    let Ok(mut pty) = ptys.get_mut(ev.entity) else {
        return;
    };
    if let Err(e) = pty.resize(ev.cols, ev.rows) {
        tracing::warn!(?e, cols = ev.cols, rows = ev.rows, "TerminalResize failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::unbounded;
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};

    /// Opens a real PTY pair, wires it into a `PtyHandle`, triggers
    /// `TerminalResize`, then reads the master's size back and asserts it
    /// reflects the requested cols/rows.
    #[test]
    fn resize_reaches_pty() {
        let pty_system = native_pty_system();
        let pty_pair = pty_system
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("open pty pair");

        let cmd = CommandBuilder::new("cat");
        let child = pty_pair.slave.spawn_command(cmd).expect("spawn cat");
        let child_killer = child.clone_killer();
        drop(pty_pair.slave);

        let writer = pty_pair.master.take_writer().expect("take writer");
        let size_probe = pty_pair.master.try_clone_reader();
        assert!(size_probe.is_ok(), "master supports cloning a reader");

        let (_chunk_tx, chunk_rx) = unbounded::<Vec<u8>>();
        let (_exit_tx, exit_rx) = unbounded::<Option<i32>>();

        let initial = pty_pair.master.get_size().expect("read initial size");
        assert_eq!((initial.cols, initial.rows), (80, 24), "born at 80x24");

        let pty_handle = PtyHandle::new(pty_pair.master, writer, chunk_rx, exit_rx, child_killer);

        let mut app = App::new();
        app.add_plugins(ResizePlugin);
        let entity = app.world_mut().spawn(pty_handle).id();

        app.world_mut().trigger(TerminalResize {
            entity,
            cols: 120,
            rows: 40,
        });
        app.update();

        let after = app
            .world()
            .entity(entity)
            .get::<PtyHandle>()
            .expect("handle present")
            .master_size()
            .expect("read size after resize");
        assert_eq!(
            (after.cols, after.rows),
            (120, 40),
            "PTY master resized to the requested cols/rows",
        );

        app.world_mut().despawn(entity);
    }

    /// Triggering `TerminalResize` on an entity with no `PtyHandle` must be a
    /// safe no-op — the observer returns early without panicking.
    #[test]
    fn resize_no_pty_is_safe_noop() {
        let mut app = App::new();
        app.add_plugins(ResizePlugin);

        let e = app.world_mut().spawn_empty().id();
        app.world_mut().trigger(TerminalResize {
            entity: e,
            cols: 10,
            rows: 5,
        });
        app.update();
    }
}
