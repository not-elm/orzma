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

async fn next_binary(rx: &mut Receiver<WireMessage>, timeout: Duration) -> Option<Bytes> {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(50), rx.recv()).await {
            Ok(Ok(WireMessage::Binary { encoded, .. })) => return Some(encoded),
            Ok(Ok(_)) => continue,
            Ok(Err(_)) => return None,
            Err(_) => continue,
        }
    }
    None
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

#[tokio::test]
async fn bursty_bulk_output_capped_at_max_cap() {
    let (svc, aid) = spawn_test_service(80, 24).await;
    let chunk_tx = svc.vt_chunk_sender_for_test(&aid).await.unwrap();
    let mut rx = svc.subscribe_wire_broadcast(&aid).await.unwrap();

    let _ = drain_binary_count(&mut rx, Duration::from_millis(200)).await;

    // 50 chunks at 1ms intervals = 50ms of input.
    for i in 0..50u8 {
        let payload = format!("\x1b[{};1HX{i}", (i % 24) + 1);
        chunk_tx
            .send(Bytes::from(payload.into_bytes()))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    let count = drain_binary_count(&mut rx, Duration::from_millis(200)).await;

    // Without coalescing this would be 50 frames. With idle=3ms / max-cap=12ms
    // over 50ms of input, expect at most ceil(50 / 12) + slack = ~6 frames.
    assert!(
        (1..=8).contains(&count),
        "expected 1-8 coalesced frames for 50-chunk burst, got {count}"
    );

    svc.kill(&aid).await.unwrap();
}

#[tokio::test]
async fn resize_emits_snapshot_immediately() {
    use ozmux_terminal::vt::RenderFrame;

    let (svc, aid) = spawn_test_service(80, 24).await;
    let mut rx = svc.subscribe_wire_broadcast(&aid).await.unwrap();

    let _ = next_binary(&mut rx, Duration::from_millis(500)).await;

    let t0 = tokio::time::Instant::now();
    svc.resize(&aid, 100, 30).await.unwrap();
    let bytes = next_binary(&mut rx, Duration::from_millis(100))
        .await
        .expect("resize must emit a binary frame");
    let elapsed = t0.elapsed();

    let frame: RenderFrame = rmp_serde::from_slice(&bytes).unwrap();
    assert!(matches!(frame, RenderFrame::Snapshot(_)));
    assert!(
        elapsed < Duration::from_millis(15),
        "resize must hit immediate-flush path, took {elapsed:?}"
    );

    svc.kill(&aid).await.unwrap();
}

#[tokio::test]
async fn bootstrap_snapshot_emits_immediately() {
    use ozmux_terminal::vt::RenderFrame;

    let (svc, aid) = spawn_test_service(20, 5).await;
    let chunk_tx = svc.vt_chunk_sender_for_test(&aid).await.unwrap();
    let mut rx = svc.subscribe_wire_broadcast(&aid).await.unwrap();

    let t0 = tokio::time::Instant::now();
    chunk_tx.send(Bytes::from_static(b"hello")).await.unwrap();
    let bytes = next_binary(&mut rx, Duration::from_millis(100))
        .await
        .expect("bootstrap snapshot must arrive");
    let elapsed = t0.elapsed();

    let frame: RenderFrame = rmp_serde::from_slice(&bytes).unwrap();
    assert!(matches!(frame, RenderFrame::Snapshot(_)));
    // The bootstrap path should be immediate (< IDLE) — generous slack for
    // CI scheduler jitter.
    assert!(
        elapsed < Duration::from_millis(15),
        "bootstrap must hit immediate-flush path, took {elapsed:?}"
    );

    svc.kill(&aid).await.unwrap();
}

#[tokio::test]
async fn single_char_echo_after_user_input_is_immediate() {
    let (svc, aid) = spawn_test_service(20, 5).await;
    let chunk_tx = svc.vt_chunk_sender_for_test(&aid).await.unwrap();
    let mut rx = svc.subscribe_wire_broadcast(&aid).await.unwrap();

    // Drain bootstrap + any shell prompt.
    let _ = drain_binary_count(&mut rx, Duration::from_millis(200)).await;

    // Simulate "user typed x" — TerminalService::write sets the flag.
    svc.write(&aid, b"x").await.unwrap();

    // Simulate the shell echoing 'x' back (single-row damage).
    let t0 = tokio::time::Instant::now();
    chunk_tx.send(Bytes::from_static(b"x")).await.unwrap();

    let _bytes = next_binary(&mut rx, Duration::from_millis(100))
        .await
        .expect("echo must produce a delta frame");
    let elapsed = t0.elapsed();

    assert!(
        elapsed < Duration::from_millis(15),
        "single-char echo must be immediate, took {elapsed:?}"
    );

    svc.kill(&aid).await.unwrap();
}

#[tokio::test]
async fn pending_user_input_is_one_shot() {
    use ozmux_terminal::vt::Coalescer;

    let (svc, aid) = spawn_test_service(80, 24).await;
    let chunk_tx = svc.vt_chunk_sender_for_test(&aid).await.unwrap();
    let mut rx = svc.subscribe_wire_broadcast(&aid).await.unwrap();

    let _ = drain_binary_count(&mut rx, Duration::from_millis(200)).await;

    svc.write(&aid, b"x").await.unwrap();
    // First chunk: single-row echo with flag set — must be immediate.
    chunk_tx.send(Bytes::from_static(b"x")).await.unwrap();
    let _ = next_binary(&mut rx, Duration::from_millis(100))
        .await
        .expect("immediate echo");

    // Second batch: many rows of bulk output AFTER flag consumed.
    // If `pending_user_input` were TTL-based, this would also flush
    // immediately. Under one-shot semantics, the flag is already false,
    // and the > 1-row damage forces coalescing (delay >= IDLE).
    let bulk: String = (1..=10u8)
        .map(|i| format!("\x1b[{i};1HROW"))
        .collect::<String>();

    let t0 = tokio::time::Instant::now();
    chunk_tx.send(Bytes::from(bulk.into_bytes())).await.unwrap();
    let _ = next_binary(&mut rx, Duration::from_millis(100))
        .await
        .expect("bulk frame must arrive");
    let elapsed = t0.elapsed();

    assert!(
        elapsed >= Coalescer::IDLE,
        "bulk output after consumed flag must coalesce (waited {elapsed:?}, IDLE={:?})",
        Coalescer::IDLE
    );

    svc.kill(&aid).await.unwrap();
}

async fn drain_binary_frames(rx: &mut Receiver<WireMessage>, settle: Duration) -> Vec<Bytes> {
    let mut frames = Vec::new();
    let deadline = tokio::time::Instant::now() + settle;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(50), rx.recv()).await {
            Ok(Ok(WireMessage::Binary { encoded, .. })) => frames.push(encoded),
            Ok(Ok(_)) => continue,
            Ok(Err(_)) => break,
            Err(_) => continue,
        }
    }
    frames
}

#[tokio::test]
async fn alt_screen_entry_chunk_split_does_not_emit_blank_snapshot() {
    use ozmux_terminal::vt::RenderFrame;

    let (svc, aid) = spawn_test_service(80, 24).await;
    let chunk_tx = svc.vt_chunk_sender_for_test(&aid).await.unwrap();
    let mut rx = svc.subscribe_wire_broadcast(&aid).await.unwrap();

    // Drain bootstrap.
    let _ = drain_binary_count(&mut rx, Duration::from_millis(200)).await;

    // Chunk 1: alt-screen entry + clear + home — no row contents yet.
    // This triggers TermDamage::Full. Pre-fix bridge immediate-flushes here,
    // emitting a blank snapshot before row content arrives.
    chunk_tx
        .send(Bytes::from_static(b"\x1b[?1049h\x1b[2J\x1b[H"))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(2)).await;
    // Chunk 2: row contents.
    chunk_tx
        .send(Bytes::from_static(b"row0\r\nrow1\r\nrow2\r\nrow3\r\nrow4"))
        .await
        .unwrap();

    // Wait for coalescer max-cap (12ms) + slack.
    let frames = drain_binary_frames(&mut rx, Duration::from_millis(100)).await;

    assert_eq!(
        frames.len(),
        1,
        "expected exactly 1 coalesced frame for alt-screen entry + content, got {}",
        frames.len()
    );

    let frame: RenderFrame = rmp_serde::from_slice(&frames[0]).unwrap();
    let RenderFrame::Snapshot(snap) = frame else {
        panic!("expected snapshot for Full damage path, got delta");
    };

    let row0_text: String = snap.rows_data[0]
        .runs
        .iter()
        .flat_map(|r| r.text.chars())
        .collect();
    assert!(
        row0_text.starts_with("row0"),
        "row 0 must contain row0 content (chunk-split must not emit blank snapshot), got {row0_text:?}"
    );

    svc.kill(&aid).await.unwrap();
}

#[tokio::test]
async fn pty_chunks_are_drained_even_when_emit_lags() {
    let (svc, aid) = spawn_test_service(80, 24).await;
    let chunk_tx = svc.vt_chunk_sender_for_test(&aid).await.unwrap();

    // Subscribe so the broadcast channel is being drained by SOMETHING (without
    // a subscriber, the broadcast::Sender may silently no-op). But we don't
    // need to read the frames — we just want to confirm Term advances.
    let _rx = svc.subscribe_wire_broadcast(&aid).await.unwrap();

    // Send 200 chunks fast (channel cap is 128 with try_send drop on the reader,
    // but we use `send().await` which blocks instead of dropping).
    for i in 0..200u32 {
        let payload = format!("\x1b[1;1H{i}");
        chunk_tx
            .send(Bytes::from(payload.into_bytes()))
            .await
            .unwrap();
    }

    // Give the bridge time to drain.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // The Term's row 0 should reflect the LAST write (199), not the first (0).
    let row0_text: String = svc
        .inspect_row(&aid, 0, 80)
        .await
        .expect("inspect_row must succeed");
    assert!(
        row0_text.trim_start_matches(' ').starts_with("199"),
        "Term must reflect late chunks; row0={row0_text:?}"
    );

    svc.kill(&aid).await.unwrap();
}
