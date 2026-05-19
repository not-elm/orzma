//! Public replay API: drive PTY tapes through the VT bridge and broadcasts
//! through subscribers under deterministic time control.
//!
//! Both functions are `async fn` — call from a tokio runtime with
//! `start_paused(true)` per spec Section 6 "Determinism boundary".
use crate::testing::player::TapePlayer;
use crate::testing::tape::Tape;
use crate::vt::bridge::{BridgeConfig, VtState, run_bridge_task};
use crate::vt::frame_ring::WireMessage;
use crate::vt::listener::{ControlFrame, DropCounter, ReplyFrame, TermListener};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

// Re-export so callers don't need to know about the internal `player` module.
pub use crate::testing::player::ReplayMode;

/// Errors `feed_pty_tape` and `stream_wire_to_subscriber` can return.
#[derive(Debug, thiserror::Error)]
pub enum ReplayError {
    /// The bridge's WireMessage broadcast channel closed before the tape
    /// finished playing (the bridge task exited prematurely).
    #[error("bridge channel closed before tape completion")]
    BridgeClosed,
    /// The bridge task panicked. The wrapped `JoinError` carries the panic
    /// info if recoverable.
    #[error("bridge task panicked: {0}")]
    BridgeTaskPanicked(#[from] tokio::task::JoinError),
    /// Tape format / load error propagated.
    #[error("tape format: {0}")]
    TapeFormat(#[from] crate::testing::tape::TapeError),
    /// Subscriber's broadcast receiver fell behind — capacity sizing is
    /// broken in the caller (`feed_pty_tape` internally sizes from manifest
    /// + slack so this should not fire).
    #[error(
        "subscriber lagged at seq {first_lost_seq}: {skipped} messages dropped — capacity sizing is broken"
    )]
    SubscriberLagged { first_lost_seq: u32, skipped: u64 },
    /// `TapePlayer` failed to send into the bridge mpsc (bridge dropped the
    /// receiver early).
    #[error("player: {0}")]
    Player(#[from] crate::testing::player::PlayerError),
}

/// Per-message outcome reported by `stream_wire_to_subscriber`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecvOutcome {
    /// Received successfully with the given `seq`.
    Ok { seq: u32 },
    /// Receiver lagged behind by `skipped` messages.
    Lagged { skipped: u64 },
}

/// Feeds a PTY tape through a VT bridge in determinism mode and returns
/// the WireMessage stream a single broadcast subscriber would observe.
///
/// # Determinism contract (caller's responsibility)
///
/// Caller MUST be in a `current_thread` tokio runtime with
/// `start_paused(true)`. See spec Section 6.
pub async fn feed_pty_tape(tape: &Tape, mode: ReplayMode) -> Result<Vec<WireMessage>, ReplayError> {
    #[cfg(debug_assertions)]
    {
        let t0 = tokio::time::Instant::now();
        tokio::task::yield_now().await;
        let t1 = tokio::time::Instant::now();
        debug_assert!(
            t1.duration_since(t0).is_zero(),
            "feed_pty_tape requires tokio start_paused(true) — see spec Section 6"
        );
    }

    let cfg = BridgeConfig {
        coalesce: false,
        spawn_gauge: false,
    };
    let capacity = tape.estimated_total_wire_messages() + 16;

    let (pty_tx, pty_rx) = mpsc::channel::<bytes::Bytes>(64);
    let (wire_tx, mut wire_rx) = broadcast::channel::<WireMessage>(capacity);

    let (reply_tx, reply_rx) = mpsc::unbounded_channel::<ReplyFrame>();
    let (control_tx, control_rx) = mpsc::channel::<ControlFrame>(64);
    let listener = TermListener {
        reply_tx,
        control_tx,
        drop_counter: Arc::new(DropCounter::new()),
    };
    let vt_state = Arc::new(std::sync::Mutex::new(VtState::new(
        80,
        24,
        listener,
        wire_tx.clone(),
    )));
    let cancel = CancellationToken::new();
    let (title_tx, _title_rx) = broadcast::channel(8);

    let bridge_handle = tokio::spawn(run_bridge_task(
        vt_state.clone(),
        pty_rx,
        reply_rx,
        control_rx,
        None,
        title_tx,
        cancel,
        cfg,
    ));

    TapePlayer::new(pty_tx.clone(), mode).play(tape).await?;
    drop(pty_tx);
    // NOTE: drop the wire_tx clone we own so the bridge becomes the sole
    // sender; when its loop exits (pty_rx closed → recv returns None →
    // break), all senders are dropped and the receiver returns Closed.
    drop(wire_tx);

    let mut out = Vec::with_capacity(capacity);
    loop {
        match wire_rx.recv().await {
            Ok(msg) => out.push(msg),
            Err(broadcast::error::RecvError::Closed) => break,
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                let first_lost_seq = match out.last() {
                    Some(WireMessage::Binary { seq, .. }) => seq + 1,
                    _ => 0,
                };
                return Err(ReplayError::SubscriberLagged {
                    first_lost_seq,
                    skipped,
                });
            }
        }
    }

    bridge_handle
        .await
        .map_err(ReplayError::BridgeTaskPanicked)?;
    Ok(out)
}

/// Drives the wire layer alone: pushes pre-built messages into a broadcast
/// and reports per-message receive outcomes for a single subscriber.
///
/// `capacity` is intentional — set small to exercise `Lagged` paths.
// TODO: PR-B fills in the producer-side variants (interleaved push/drop,
// fixed-rate pump) once the depth-gauge work lands.
pub async fn stream_wire_to_subscriber(
    messages: &[WireMessage],
    capacity: usize,
) -> Vec<RecvOutcome> {
    let (tx, mut rx) = broadcast::channel::<WireMessage>(capacity);
    let messages_owned: Vec<WireMessage> = messages.to_vec();
    let producer = tokio::spawn(async move {
        for msg in messages_owned {
            let _ = tx.send(msg);
        }
        drop(tx);
    });
    // NOTE: drop the JoinHandle (don't await) — the producer's lifetime is
    // tied to the receiver's drain loop below, which sees Closed when the
    // task finishes and the tx clone goes out of scope.
    drop(producer);

    let mut outcomes = Vec::new();
    loop {
        match rx.recv().await {
            Ok(WireMessage::Binary { seq, .. }) => outcomes.push(RecvOutcome::Ok { seq }),
            Ok(_) => outcomes.push(RecvOutcome::Ok { seq: 0 }),
            Err(broadcast::error::RecvError::Closed) => break,
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                outcomes.push(RecvOutcome::Lagged { skipped });
            }
        }
    }
    outcomes
}
