//! Integration tests for scrollback navigation (TerminalService::scroll).

use ozmux_multiplexer::{ActivityId, PaneId};
use ozmux_terminal::vt::RenderFrame;
use ozmux_terminal::{FrameSubscription, SpawnOptions, TerminalEvent, TerminalService};
use std::time::Duration;
use tokio::time::Instant;

async fn spawn_shell(svc: &TerminalService) -> ActivityId {
    let aid = ActivityId::new();
    svc.spawn(
        PaneId::new(),
        aid.clone(),
        SpawnOptions {
            cols: 20,
            rows: 5,
            shell: "/bin/sh".to_string(),
            cwd: None,
            window_id: None,
            session_id: None,
        },
    )
    .await
    .unwrap();
    aid
}

async fn pump_until_idle(svc: &TerminalService, aid: &ActivityId, ms: u64) {
    let (_, mut rx) = svc.snapshot_and_subscribe(aid).await.unwrap();
    let deadline = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(50), rx.recv()).await {
            Ok(Ok(TerminalEvent::Data { .. })) => continue,
            Ok(Ok(TerminalEvent::Exit { .. })) | Ok(Err(_)) | Err(_) => return,
        }
    }
}

#[tokio::test]
async fn scroll_advances_display_offset() {
    let svc = TerminalService::default();
    let aid = spawn_shell(&svc).await;

    let mut cmd = String::new();
    for i in 0..30 {
        cmd.push_str(&format!("echo line{i}\n"));
    }
    svc.write(&aid, cmd.as_bytes()).await.unwrap();
    pump_until_idle(&svc, &aid, 1500).await;

    svc.scroll(&aid, 10).await.unwrap();
    // NOTE: synthetic wakeup is already sent by scroll(); small sleep lets the
    // bridge task process it before we take the snapshot.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let sub = svc.subscribe_frames(&aid, None).await.unwrap();
    let snap_bytes = match sub {
        FrameSubscription::FreshSnapshot { snapshot, .. } => snapshot,
        FrameSubscription::ResumeReplay { .. } => panic!("expected fresh snapshot"),
    };
    let frame: RenderFrame = rmp_serde::from_slice(&snap_bytes).expect("decode");
    let snap = match frame {
        RenderFrame::Snapshot(s) => s,
        _ => panic!("expected Snapshot variant"),
    };
    assert!(
        snap.display_offset > 0,
        "display_offset should advance past 0"
    );
    assert!(snap.history_size >= snap.display_offset);

    svc.kill(&aid).await.unwrap();
}

#[tokio::test]
async fn scroll_to_bottom_resets_display_offset() {
    let svc = TerminalService::default();
    let aid = spawn_shell(&svc).await;

    let mut cmd = String::new();
    for i in 0..30 {
        cmd.push_str(&format!("echo line{i}\n"));
    }
    svc.write(&aid, cmd.as_bytes()).await.unwrap();
    pump_until_idle(&svc, &aid, 1500).await;

    svc.scroll(&aid, 10).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    svc.scroll_to_bottom(&aid).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let sub = svc.subscribe_frames(&aid, None).await.unwrap();
    let snap_bytes = match sub {
        FrameSubscription::FreshSnapshot { snapshot, .. } => snapshot,
        _ => panic!("expected snapshot"),
    };
    let frame: RenderFrame = rmp_serde::from_slice(&snap_bytes).unwrap();
    let snap = match frame {
        RenderFrame::Snapshot(s) => s,
        _ => panic!("expected Snapshot"),
    };
    assert_eq!(snap.display_offset, 0);

    svc.kill(&aid).await.unwrap();
}
