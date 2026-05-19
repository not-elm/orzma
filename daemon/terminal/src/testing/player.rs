//! Drives a Tape into a bridge's `mpsc::Sender<Bytes>`, preserving chunk
//! boundaries.
use crate::testing::tape::{Tape, TapeRecord};
use bytes::Bytes;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;

/// How `TapePlayer::play` paces chunks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayMode {
    /// Send chunks back-to-back, no sleeping. Use for throughput benches
    /// where the timing distribution is determined by the consumer, not
    /// the original capture pacing.
    Immediate,
    /// Honor each chunk's `ts_ns_offset` via `tokio::time::sleep_until`.
    /// Use for `?replay=` query (matches original session pacing so the
    /// rendered output looks like the original interactive session).
    Timed,
}

/// Errors `TapePlayer::play` can return.
#[derive(Debug, thiserror::Error)]
pub enum PlayerError {
    /// The bridge's mpsc receiver was dropped before the tape finished
    /// playing. Indicates the bridge task exited prematurely.
    #[error("send failed (bridge mpsc receiver closed)")]
    Send,
}

/// Streams a tape's chunks into a bridge `mpsc::Sender<Bytes>`.
pub struct TapePlayer {
    pty_tx: mpsc::Sender<Bytes>,
    mode: ReplayMode,
}

impl TapePlayer {
    /// Constructs a player attached to the given mpsc sender.
    pub fn new(pty_tx: mpsc::Sender<Bytes>, mode: ReplayMode) -> Self {
        Self { pty_tx, mode }
    }

    /// Plays the tape's records through the sender.
    ///
    /// Each `TapeRecord` is sent as a single `mpsc::send` call — chunk
    /// boundaries are preserved exactly as captured, so the bridge sees
    /// the same chunk shape as the production PTY reader.
    pub async fn play(&self, tape: &Tape) -> Result<(), PlayerError> {
        let start = Instant::now();
        for record in &tape.records {
            if self.mode == ReplayMode::Timed {
                let deadline = start + Duration::from_nanos(record.ts_ns_offset);
                tokio::time::sleep_until(deadline).await;
            }
            self.pty_tx
                .send(Bytes::copy_from_slice(&record.bytes))
                .await
                .map_err(|_| PlayerError::Send)?;
        }
        Ok(())
    }
}

impl From<&TapeRecord> for Bytes {
    fn from(r: &TapeRecord) -> Self {
        Bytes::copy_from_slice(&r.bytes)
    }
}
