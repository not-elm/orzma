//! Integration tests for the Phase 2A wire emit path.

use bytes::Bytes;
use ozmux_multiplexer::{ActivityId, PaneId};
use ozmux_terminal::vt::{FrameSnapshot, RenderFrame, WireMessage};
use ozmux_terminal::{SpawnOptions, TerminalService};

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
    // CAT-007: mode transitions are inlined into the next binary FrameDelta
    // rather than emitted as a separate WireMessage::Text frame.
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
