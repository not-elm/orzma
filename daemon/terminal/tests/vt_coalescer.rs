//! Integration tests for the per-bridge frame coalescer.
//!
//! These tests drive `TerminalService` with real PTYs and assert wire-frame
//! invariants under timing pressure. Real PTYs are used (not mocks) so the
//! tests exercise the same code path production hits.

use std::time::Duration;

use bytes::Bytes;
use ozmux_multiplexer::{ActivityId, PaneId};
use ozmux_terminal::vt::WireMessage;
use ozmux_terminal::{SpawnOptions, TerminalService};
use tokio::sync::broadcast::Receiver;

async fn spawn_test_service(cols: u16, rows: u16) -> (TerminalService, ActivityId) {
    let svc = TerminalService::default();
    let pane = PaneId::new();
    let aid = ActivityId::new();
    svc.spawn(
        pane,
        aid.clone(),
        SpawnOptions {
            cols,
            rows,
            shell: "/bin/sh".to_string(),
            cwd: None,
            window_id: None,
            session_id: None,
        },
    )
    .await
    .unwrap();
    (svc, aid)
}

async fn drain_binary_count(rx: &mut Receiver<WireMessage>, settle: Duration) -> usize {
    let mut count = 0;
    let deadline = tokio::time::Instant::now() + settle;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(50), rx.recv()).await {
            Ok(Ok(WireMessage::Binary { .. })) => count += 1,
            Ok(Ok(_)) => continue,
            Ok(Err(_)) => break,
            Err(_) => continue,
        }
    }
    count
}

#[tokio::test]
async fn clear_then_fill_emits_one_frame_after_window() {
    let (svc, aid) = spawn_test_service(80, 24).await;
    let chunk_tx = svc.vt_chunk_sender_for_test(&aid).await.unwrap();
    let mut rx = svc.subscribe_wire_broadcast(&aid).await.unwrap();

    // Drain the bootstrap snapshot (and shell prompt) — settle for 200ms.
    let _ = drain_binary_count(&mut rx, Duration::from_millis(200)).await;

    // Send two damage-inducing chunks 2ms apart, directly to the bridge.
    // The current (pre-coalescer) bridge emits one frame per chunk → 2 frames.
    // The coalescer will fold them into 1 frame after the idle window (3ms).
    chunk_tx
        .send(Bytes::from_static(b"\x1b[1;1HAAA"))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(2)).await;
    chunk_tx
        .send(Bytes::from_static(b"\x1b[2;1HBBB"))
        .await
        .unwrap();

    // Wait for coalescer (max-cap 12ms) + slack for the bridge to wake.
    let count = drain_binary_count(&mut rx, Duration::from_millis(100)).await;

    assert_eq!(
        count, 1,
        "expected exactly 1 coalesced frame after clear-then-fill, got {count}"
    );

    svc.kill(&aid).await.unwrap();
}
