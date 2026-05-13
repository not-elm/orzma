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
