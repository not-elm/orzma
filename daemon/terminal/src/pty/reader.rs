//! Background OS thread + tokio bridge that drains the PTY master.

use crate::event::TerminalEvent;
use crate::pty::scrollback::ScrollbackBuffer;
use bytes::Bytes;
use portable_pty::Child;
use std::io::Read;
use tokio::sync::{broadcast, mpsc};

/// Spawn a dedicated OS thread for blocking PTY reads, plus a tokio bridge
/// task that pushes data to the scrollback and broadcasts under the same
/// lock (race-free with snapshot_and_subscribe).
///
/// The OS thread is preferred over `tokio::spawn` here because `read()` is
/// blocking and would otherwise occupy a tokio worker.
pub(crate) fn spawn_pty_reader(
    mut reader: Box<dyn Read + Send>,
    mut child: Box<dyn Child + Send + Sync>,
    scrollback: ScrollbackBuffer,
    event_sender: broadcast::Sender<TerminalEvent>,
    vt_chunk_tx: mpsc::Sender<Bytes>,
) {
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match std::io::Read::read(&mut reader, &mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let chunk = buf[..n].to_vec();
                    // NOTE: blocking_send applies backpressure to the read thread
                    // when the bounded vt_chunk channel is full. `try_send` would
                    // silently drop bytes under sustained PTY output (cat large
                    // file, busy TUI repaint), causing terminal display corruption
                    // since the VT parser never sees the dropped chunks. The OS
                    // reader thread is synchronous and is the right place to
                    // exert backpressure all the way back to the PTY.
                    // Err here means the channel is closed (bridge task exited);
                    // break the loop in that case.
                    if vt_chunk_tx
                        .blocking_send(Bytes::copy_from_slice(&chunk))
                        .is_err()
                    {
                        break;
                    }
                    scrollback.push(&chunk);
                    let _ = event_sender.send(TerminalEvent::Data { buffer: chunk });
                }
                Err(_) => break,
            }
        }
        let code = child.wait().ok().map(|s| s.exit_code() as i32);
        let _ = event_sender.send(TerminalEvent::Exit { code });
    });
}
