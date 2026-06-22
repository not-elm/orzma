//! `TerminalRawWrite` — writes raw bytes to a terminal's PTY without the
//! `pending_user_input` echo/coalescing side effect of `TerminalHandle::write`.

use crate::pty::PtyHandle;
use bevy::ecs::entity::Entity;
use bevy::ecs::event::EntityEvent;
use bevy::ecs::observer::On;
use bevy::ecs::system::Query;
use bevy::prelude::*;

/// Requests a raw, side-effect-free write of `bytes` to `entity`'s PTY.
///
/// Used by the tmux adoption bridge to send control-mode commands over the
/// adopted gateway PTY. Unlike `TerminalHandle::write`, it does not set
/// `pending_user_input`, so it never perturbs the VT echo/coalescing state.
#[derive(EntityEvent)]
pub struct TerminalRawWrite {
    /// Target terminal entity (must own a `PtyHandle`).
    #[event_target]
    pub entity: Entity,
    /// Bytes to write verbatim to the PTY master.
    pub bytes: Vec<u8>,
}

/// Registers the `TerminalRawWrite` observer.
pub struct RawWritePlugin;

impl Plugin for RawWritePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_raw_write);
    }
}

fn on_raw_write(ev: On<TerminalRawWrite>, mut ptys: Query<&mut PtyHandle>) {
    let Ok(mut pty) = ptys.get_mut(ev.entity) else {
        return;
    };
    if let Err(e) = pty.write_all(&ev.bytes) {
        tracing::warn!(?e, "TerminalRawWrite failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::unbounded;
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};
    use std::io::Read;
    use std::time::Duration;

    /// Opens a PTY pair backed by `cat`, wires it into a `PtyHandle`, triggers
    /// `TerminalRawWrite`, then drains PTY output on a background thread and
    /// asserts the written bytes arrive back (PTY terminal discipline echoes
    /// master writes to the slave reader, which `cat` then echoes back to the
    /// master).
    #[test]
    fn raw_write_reaches_pty() {
        let pty_system = native_pty_system();
        let pty_pair = pty_system
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("open pty pair");

        // Spawn `cat` — echoes stdin back on stdout, so bytes we write to the
        // master appear as PTY output on the reader.
        let cmd = CommandBuilder::new("cat");
        let child = pty_pair.slave.spawn_command(cmd).expect("spawn cat");
        let child_killer = child.clone_killer();
        drop(pty_pair.slave);

        let mut reader = pty_pair.master.try_clone_reader().expect("clone reader");
        let writer = pty_pair.master.take_writer().expect("take writer");

        let (chunk_tx, chunk_rx) = unbounded::<Vec<u8>>();
        let (exit_tx, exit_rx) = unbounded::<Option<i32>>();

        // Drain PTY output on a background thread so the synchronous write
        // path doesn't deadlock waiting for the master buffer to drain.
        let (output_tx, output_rx) = unbounded::<Vec<u8>>();
        std::thread::spawn(move || {
            let mut buf = [0u8; 256];
            loop {
                let n = match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                if output_tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });

        let pty_handle = PtyHandle::new(pty_pair.master, writer, chunk_rx, exit_rx, child_killer);

        let mut app = App::new();
        app.add_plugins(RawWritePlugin);

        let entity = app.world_mut().spawn(pty_handle).id();

        app.world_mut().trigger(TerminalRawWrite {
            entity,
            bytes: b"hello".to_vec(),
        });
        app.update();

        // Collect PTY output for up to 2 s.  PTY terminal discipline echoes
        // master writes back as output, so "hello" will appear in received.
        let mut received = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            if let Ok(chunk) = output_rx.recv_timeout(Duration::from_millis(50)) {
                received.extend_from_slice(&chunk);
            }
            if received.windows(b"hello".len()).any(|w| w == b"hello") {
                break;
            }
            if std::time::Instant::now() >= deadline {
                break;
            }
        }

        // Drop channels so the background thread exits.
        drop(chunk_tx);
        drop(exit_tx);

        assert!(
            received.windows(b"hello".len()).any(|w| w == b"hello"),
            "expected 'hello' in PTY output but got: {:?}",
            received
        );
    }

    /// Triggering `TerminalRawWrite` on an entity with no `PtyHandle` must be
    /// a safe no-op — the observer returns early without panicking.
    #[test]
    fn raw_write_no_pty_is_safe_noop() {
        let mut app = App::new();
        app.add_plugins(RawWritePlugin);

        let e = app.world_mut().spawn_empty().id();
        app.world_mut().trigger(TerminalRawWrite {
            entity: e,
            bytes: b"noop".to_vec(),
        });
        app.update();
    }
}
