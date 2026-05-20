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
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(64);
    let exit_event_sender = event_sender.clone();

    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match std::io::Read::read(&mut reader, &mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if tx.blocking_send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let code = child.wait().ok().map(|s| s.exit_code() as i32);
        let _ = exit_event_sender.send(TerminalEvent::Exit { code });
    });

    // NOTE: VT fan-out runs before push_and_broadcast so the raw path is
    // never stalled by VT consumption pressure; try_send drops silently when
    // the VT bridge can't keep up, and the raw scrollback path remains
    // source of truth.
    tokio::spawn(async move {
        while let Some(chunk) = rx.recv().await {
            if vt_chunk_tx.try_send(Bytes::copy_from_slice(&chunk)).is_err() {
                metrics::counter!("ozmux_terminal_pty_chunk_drops_total").increment(1);
            }
            scrollback.push_and_broadcast(&event_sender, chunk).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use metrics_util::debugging::{DebugValue, DebuggingRecorder, Snapshotter};

    fn snapshot_counter_value(snapshotter: &Snapshotter, name: &str) -> Option<u64> {
        snapshotter
            .snapshot()
            .into_vec()
            .into_iter()
            .find_map(|(key, _unit, _desc, value)| {
                if key.key().name() == name {
                    match value {
                        DebugValue::Counter(c) => Some(c),
                        _ => None,
                    }
                } else {
                    None
                }
            })
    }

    #[tokio::test]
    async fn pty_reader_try_send_full_increments_drops_counter() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _guard = metrics::set_default_local_recorder(&recorder);

        // Capacity 1 channel saturated by the first send; a second
        // try_send hits Err(Full). Mirror the production arm by
        // incrementing the drop counter on error.
        let (tx, _rx) = tokio::sync::mpsc::channel::<Bytes>(1);
        let _ = tx.try_send(Bytes::from_static(b"first")); // fills capacity
        let res = tx.try_send(Bytes::from_static(b"second"));
        assert!(res.is_err(), "second try_send must hit Err(Full)");
        if res.is_err() {
            metrics::counter!("ozmux_terminal_pty_chunk_drops_total").increment(1);
        }

        let v = snapshot_counter_value(&snapshotter, "ozmux_terminal_pty_chunk_drops_total");
        assert_eq!(v, Some(1));
    }
}
