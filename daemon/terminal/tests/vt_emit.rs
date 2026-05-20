//! Integration tests for the Phase 2A wire emit path.

use bytes::Bytes;
use ozmux_multiplexer::{ActivityId, PaneId};
use ozmux_terminal::vt::{FrameDelta, FrameSnapshot, RenderFrame, WireMessage};
use ozmux_terminal::{SpawnOptions, TerminalService};
use std::time::Duration;
use tokio::sync::broadcast::Receiver;

#[tokio::test]
async fn first_chunk_emits_a_snapshot_on_broadcast() {
    let svc = TerminalService::default();
    let pane = PaneId::new();
    let aid = ActivityId::new();
    svc.spawn(
        pane,
        aid.clone(),
        SpawnOptions {
            cols: 10,
            rows: 3,
            shell: "/bin/sh".to_string(),
            cwd: None,
            window_id: None,
            session_id: None,
        },
    )
    .await
    .unwrap();

    // Subscribe BEFORE input so the first emit is captured.
    let mut rx = svc.subscribe_wire_broadcast(&aid).await.unwrap();

    // Trigger emission.
    svc.write(&aid, b"hi\n").await.unwrap();

    // Drain up to 2s waiting for the first Binary frame.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    let mut got: Option<Bytes> = None;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await {
            Ok(Ok(WireMessage::Binary { encoded, .. })) => {
                got = Some(encoded);
                break;
            }
            Ok(Ok(_)) => continue,
            Ok(Err(_)) => break,
            Err(_) => continue,
        }
    }
    let bytes = got.expect("Binary frame must arrive");

    // Decode and assert it's a snapshot with reason Initial.
    let frame: RenderFrame = rmp_serde::from_slice(&bytes).expect("decode");
    match frame {
        RenderFrame::Snapshot(FrameSnapshot { reason, .. }) => {
            assert_eq!(format!("{reason:?}"), "Initial");
        }
        RenderFrame::Delta(_) => panic!("expected snapshot, got delta"),
    }
    svc.kill(&aid).await.unwrap();
}

#[tokio::test]
async fn second_chunk_emits_a_delta() {
    let svc = TerminalService::default();
    let pane = PaneId::new();
    let aid = ActivityId::new();
    svc.spawn(
        pane,
        aid.clone(),
        SpawnOptions {
            cols: 80,
            rows: 24,
            shell: "/bin/sh".to_string(),
            cwd: None,
            window_id: None,
            session_id: None,
        },
    )
    .await
    .unwrap();

    let mut rx = svc.subscribe_wire_broadcast(&aid).await.unwrap();

    svc.write(&aid, b"echo first\n").await.unwrap();

    // Skim past the first Binary (snapshot).
    let _ = collect_binary(&mut rx, std::time::Duration::from_secs(2)).await;

    svc.write(&aid, b"echo second\n").await.unwrap();
    let bytes = collect_binary(&mut rx, std::time::Duration::from_secs(2))
        .await
        .expect("second Binary");
    let frame: RenderFrame = rmp_serde::from_slice(&bytes).unwrap();
    assert!(matches!(frame, RenderFrame::Delta(_)));
    svc.kill(&aid).await.unwrap();
}

#[tokio::test]
async fn mode_change_inlines_into_next_binary_delta() {
    // Mode transitions are inlined into the next binary FrameDelta rather
    // than emitted as a separate WireMessage::Text frame.
    //
    // Bracketed-paste mode (\x1b[?2004h) toggles a mode bit without triggering
    // DirtyRows::Full, so the emit path is a Delta (not a Snapshot). The
    // trailing printable byte 'X' dirties one row, causing the bridge to flush
    // a Delta that carries modes_added=["bracketed-paste"].
    let svc = TerminalService::default();
    let pane = PaneId::new();
    let aid = ActivityId::new();
    svc.spawn(
        pane,
        aid.clone(),
        SpawnOptions {
            cols: 80,
            rows: 24,
            shell: "/bin/sh".to_string(),
            cwd: None,
            window_id: None,
            session_id: None,
        },
    )
    .await
    .unwrap();

    let chunk_tx = svc.vt_chunk_sender_for_test(&aid).await.unwrap();
    let mut rx = svc.subscribe_wire_broadcast(&aid).await.unwrap();

    // Drain bootstrap frames so subsequent receives are caused only by the test input.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(300);
    while tokio::time::Instant::now() < deadline {
        let _ = tokio::time::timeout(std::time::Duration::from_millis(30), rx.recv()).await;
    }

    // Send bracketed-paste enable + one printable byte in a single chunk.
    chunk_tx
        .send(bytes::Bytes::from_static(b"\x1b[?2004hX"))
        .await
        .unwrap();

    // Collect all binary frames arriving within 500 ms.
    let mut binary_frames: Vec<bytes::Bytes> = Vec::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(500);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(std::time::Duration::from_millis(30), rx.recv()).await {
            Ok(Ok(WireMessage::Binary { encoded, .. })) => binary_frames.push(encoded),
            Ok(Ok(_)) => continue,
            Ok(Err(_)) => break,
            Err(_) => continue,
        }
    }

    assert!(
        !binary_frames.is_empty(),
        "expected at least one binary frame on the broadcast"
    );

    let first_delta_with_modes = binary_frames
        .into_iter()
        .find_map(|bytes| {
            let frame: ozmux_terminal::vt::RenderFrame = rmp_serde::from_slice(&bytes).ok()?;
            match frame {
                ozmux_terminal::vt::RenderFrame::Delta(d) if !d.modes_added.is_empty() => Some(d),
                _ => None,
            }
        })
        .expect("no FrameDelta with inlined modes_added found");

    assert!(
        first_delta_with_modes
            .modes_added
            .iter()
            .any(|m| m == "bracketed-paste"),
        "modes_added should contain bracketed-paste; got {:?}",
        first_delta_with_modes.modes_added
    );

    svc.kill(&aid).await.unwrap();
}

async fn spawn_terminal(cols: u16, rows: u16) -> (TerminalService, ActivityId) {
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

async fn drain_binary_emit(rx: &mut Receiver<WireMessage>, settle: Duration) -> Vec<Bytes> {
    let mut frames = Vec::new();
    let deadline = tokio::time::Instant::now() + settle;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(30), rx.recv()).await {
            Ok(Ok(WireMessage::Binary { encoded, .. })) => frames.push(encoded),
            Ok(Ok(_)) => continue,
            Ok(Err(_)) => break,
            Err(_) => continue,
        }
    }
    frames
}

/// Writing identical content to the same row twice must cause the second
/// emit's delta to have no dirty_rows entry for that row. The hash filter
/// drops the row because it is unchanged since the previous emit.
#[tokio::test]
async fn cat005_hash_filter_drops_identical_row() {
    let (svc, aid) = spawn_terminal(80, 24).await;
    let chunk_tx = svc.vt_chunk_sender_for_test(&aid).await.unwrap();
    let mut rx = svc.subscribe_wire_broadcast(&aid).await.unwrap();

    // Drain the bootstrap snapshot and any shell output.
    drain_binary_emit(&mut rx, Duration::from_millis(300)).await;

    // First emit: write "X" at row 0, col 0. The delta goes out; row 0's
    // hash is stored in row_hashes.
    chunk_tx
        .send(Bytes::from_static(b"\x1b[1;1HX"))
        .await
        .unwrap();
    let _frames_a = drain_binary_emit(&mut rx, Duration::from_millis(200)).await;

    // Second emit: write the same byte at the same position. The hash must
    // match and the row must be suppressed.
    chunk_tx
        .send(Bytes::from_static(b"\x1b[1;1HX"))
        .await
        .unwrap();
    let frames_b = drain_binary_emit(&mut rx, Duration::from_millis(200)).await;

    let any_row0_dirty = frames_b.iter().any(|bytes| {
        let frame: RenderFrame = match rmp_serde::from_slice(bytes) {
            Ok(f) => f,
            Err(_) => return false,
        };
        match frame {
            RenderFrame::Delta(FrameDelta { dirty_rows, .. }) => {
                dirty_rows.iter().any(|r| r.row == 0)
            }
            RenderFrame::Snapshot(_) => false,
        }
    });
    assert!(
        !any_row0_dirty,
        "row 0 must be dropped by the hash filter on an identical re-write"
    );

    svc.kill(&aid).await.unwrap();
}

/// Moving the cursor within a row (without writing any new cells) changes
/// the row's hash because cursor.x is included. The row must therefore NOT
/// be suppressed — it must appear in the next delta so the client can
/// update the cursor position.
#[tokio::test]
async fn cat005_cursor_x_change_invalidates_row() {
    let (svc, aid) = spawn_terminal(80, 24).await;
    let chunk_tx = svc.vt_chunk_sender_for_test(&aid).await.unwrap();
    let mut rx = svc.subscribe_wire_broadcast(&aid).await.unwrap();

    // Drain bootstrap frames.
    drain_binary_emit(&mut rx, Duration::from_millis(300)).await;

    // Write a character at row 0, col 0 so row_hashes is seeded with the
    // hash that includes cursor.x == 0.
    chunk_tx
        .send(Bytes::from_static(b"\x1b[1;1HX"))
        .await
        .unwrap();
    let _frames_a = drain_binary_emit(&mut rx, Duration::from_millis(200)).await;

    // Move cursor to row 0, col 5 — no cell content change. The stored hash
    // for row 0 has cursor.x == 0; the new hash will have cursor.x == 5.
    chunk_tx
        .send(Bytes::from_static(b"\x1b[1;6H"))
        .await
        .unwrap();
    let frames_b = drain_binary_emit(&mut rx, Duration::from_millis(200)).await;

    // At least one frame must carry row 0 in dirty_rows (or be a snapshot),
    // confirming the hash cache did not suppress the cursor-position change.
    let row0_emitted = frames_b.iter().any(|bytes| {
        let frame: RenderFrame = match rmp_serde::from_slice(bytes) {
            Ok(f) => f,
            Err(_) => return false,
        };
        match frame {
            RenderFrame::Delta(FrameDelta { dirty_rows, .. }) => {
                dirty_rows.iter().any(|r| r.row == 0)
            }
            RenderFrame::Snapshot(_) => true,
        }
    });
    assert!(
        row0_emitted,
        "cursor.x change must invalidate the row hash so row 0 is not suppressed"
    );

    svc.kill(&aid).await.unwrap();
}

/// A Snapshot emit bulk-resets row_hashes. After a resize-triggered snapshot
/// the hash cache is cleared, so writing the same content that was there
/// before the resize must not be silently suppressed in the next delta.
#[tokio::test]
async fn cat005_snapshot_resets_hash_cache() {
    let (svc, aid) = spawn_terminal(80, 24).await;
    let chunk_tx = svc.vt_chunk_sender_for_test(&aid).await.unwrap();
    let mut rx = svc.subscribe_wire_broadcast(&aid).await.unwrap();

    // Drain bootstrap frames.
    drain_binary_emit(&mut rx, Duration::from_millis(300)).await;

    // Write "X" at row 0 and let row_hashes be populated.
    chunk_tx
        .send(Bytes::from_static(b"\x1b[1;1HX"))
        .await
        .unwrap();
    let _frames_a = drain_binary_emit(&mut rx, Duration::from_millis(200)).await;

    // Trigger a resize — this clears row_hashes and causes a Snapshot emit
    // that repopulates the cache from scratch.
    svc.resize(&aid, 80, 30).await.unwrap();
    let _frames_b = drain_binary_emit(&mut rx, Duration::from_millis(200)).await;

    // Write "X" again at the same position. If the cache were still active
    // the row would be suppressed. Because resize clears it, the row must
    // appear in the next emit.
    chunk_tx
        .send(Bytes::from_static(b"\x1b[1;1HX"))
        .await
        .unwrap();
    let frames_c = drain_binary_emit(&mut rx, Duration::from_millis(200)).await;

    assert!(
        !frames_c.is_empty(),
        "after a snapshot (resize), the hash cache was reset; re-writing the same \
         content must produce a frame, not be silently suppressed"
    );

    svc.kill(&aid).await.unwrap();
}

async fn collect_binary(
    rx: &mut tokio::sync::broadcast::Receiver<WireMessage>,
    timeout: std::time::Duration,
) -> Option<Bytes> {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await {
            Ok(Ok(WireMessage::Binary { encoded, .. })) => return Some(encoded),
            Ok(Ok(_)) => continue,
            Ok(Err(_)) => return None,
            Err(_) => continue,
        }
    }
    None
}

use ozmux_terminal::FrameSubscription;
use ozmux_terminal::vt::SnapshotReason;

#[tokio::test]
async fn resize_emits_snapshot_with_resize_reason() {
    let svc = TerminalService::default();
    let pane = PaneId::new();
    let aid = ActivityId::new();
    svc.spawn(
        pane,
        aid.clone(),
        SpawnOptions {
            cols: 80,
            rows: 24,
            shell: "/bin/sh".to_string(),
            cwd: None,
            window_id: None,
            session_id: None,
        },
    )
    .await
    .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let sub = svc.subscribe_frames(&aid, None).await.unwrap();
    let mut rx = match sub {
        FrameSubscription::FreshSnapshot { rx, .. }
        | FrameSubscription::ResumeReplay { rx, .. } => rx,
    };

    svc.resize(&aid, 80, 30).await.unwrap();

    let mut saw_resize = false;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        if let Some(b) = collect_binary(&mut rx, std::time::Duration::from_millis(200)).await {
            let f: RenderFrame = rmp_serde::from_slice(&b).unwrap();
            if let RenderFrame::Snapshot(s) = f
                && matches!(s.reason, SnapshotReason::Resize)
            {
                saw_resize = true;
                break;
            }
        }
    }
    assert!(
        saw_resize,
        "resize must trigger Snapshot {{ reason: Resize }}"
    );
    svc.kill(&aid).await.unwrap();
}

#[tokio::test]
async fn subscribe_frames_fresh_returns_snapshot_then_continues() {
    let svc = TerminalService::default();
    let pane = PaneId::new();
    let aid = ActivityId::new();
    svc.spawn(
        pane,
        aid.clone(),
        SpawnOptions {
            cols: 80,
            rows: 24,
            shell: "/bin/sh".to_string(),
            cwd: None,
            window_id: None,
            session_id: None,
        },
    )
    .await
    .unwrap();

    // Let initial snapshot/delta accumulate from shell prompt.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let sub = svc.subscribe_frames(&aid, None).await.unwrap();
    let (snapshot_bytes, mut rx) = match sub {
        FrameSubscription::FreshSnapshot { snapshot, rx } => (snapshot, rx),
        FrameSubscription::ResumeReplay { .. } => panic!("expected fresh"),
    };
    let snap: RenderFrame = rmp_serde::from_slice(&snapshot_bytes).unwrap();
    let snap_seq = match snap {
        RenderFrame::Snapshot(s) => s.seq,
        RenderFrame::Delta(_) => panic!("expected snapshot"),
    };

    // Trigger another emit.
    svc.write(&aid, b"echo gap_check\n").await.unwrap();
    let next_bytes = collect_binary(&mut rx, std::time::Duration::from_secs(3))
        .await
        .expect("next Binary after subscribe");
    let next: RenderFrame = rmp_serde::from_slice(&next_bytes).unwrap();
    let next_seq = match next {
        RenderFrame::Snapshot(ozmux_terminal::vt::FrameSnapshot { seq, .. })
        | RenderFrame::Delta(ozmux_terminal::vt::FrameDelta { seq, .. }) => seq,
    };
    assert!(
        next_seq > snap_seq,
        "next seq must be greater than snapshot seq; got {snap_seq} -> {next_seq}"
    );
    svc.kill(&aid).await.unwrap();
}

#[tokio::test]
async fn subscribe_frames_resume_with_last_seq() {
    let svc = TerminalService::default();
    let pane = PaneId::new();
    let aid = ActivityId::new();
    svc.spawn(
        pane,
        aid.clone(),
        SpawnOptions {
            cols: 80,
            rows: 24,
            shell: "/bin/sh".to_string(),
            cwd: None,
            window_id: None,
            session_id: None,
        },
    )
    .await
    .unwrap();

    // Drive several emits so the ring has content.
    for cmd in [b"echo a\n".as_slice(), b"echo b\n", b"echo c\n"] {
        svc.write(&aid, cmd).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    }

    // Resume from seq=0. Either ResumeReplay with deltas (if ring still has them)
    // or FreshSnapshot (if evicted) is acceptable.
    let sub = svc.subscribe_frames(&aid, Some(0)).await.unwrap();
    match sub {
        FrameSubscription::ResumeReplay { deltas, .. } => {
            assert!(
                !deltas.is_empty(),
                "expected at least one buffered delta when resuming from 0"
            );
        }
        FrameSubscription::FreshSnapshot { snapshot, .. } => {
            // Acceptable: confirm it's a valid snapshot.
            let _: ozmux_terminal::vt::RenderFrame = rmp_serde::from_slice(&snapshot).unwrap();
        }
    }
    svc.kill(&aid).await.unwrap();
}
