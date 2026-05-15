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

async fn read_display_offset(svc: &TerminalService, aid: &ActivityId) -> u32 {
    let sub = svc.subscribe_frames(aid, None).await.unwrap();
    let snap_bytes = match sub {
        FrameSubscription::FreshSnapshot { snapshot, .. } => snapshot,
        FrameSubscription::ResumeReplay { .. } => panic!("expected fresh snapshot"),
    };
    let frame: RenderFrame = rmp_serde::from_slice(&snap_bytes).expect("decode");
    match frame {
        RenderFrame::Snapshot(s) => s.display_offset,
        _ => panic!("expected Snapshot variant"),
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

    let offset = read_display_offset(&svc, &aid).await;
    assert!(offset > 0, "display_offset should advance past 0");

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

    let offset = read_display_offset(&svc, &aid).await;
    assert_eq!(offset, 0);

    svc.kill(&aid).await.unwrap();
}

#[tokio::test]
async fn scroll_position_locks_during_new_output() {
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

    let offset_before = read_display_offset(&svc, &aid).await;
    assert!(offset_before > 0, "scrolled back");

    svc.write(&aid, b"echo new1\necho new2\necho new3\n")
        .await
        .unwrap();
    pump_until_idle(&svc, &aid, 1500).await;

    let offset_after = read_display_offset(&svc, &aid).await;
    // NOTE: alacritty increments display_offset by the number of new history
    // lines so the visible window stays pinned to the same scrollback content.
    // The regression-guard invariant is therefore offset_after > 0 (viewport
    // did NOT snap to the live tail) and offset_after >= offset_before (the
    // lock is active, not reset).
    assert!(
        offset_after > 0,
        "scroll position must not snap to live tail during new output (expected > 0, got {offset_after})"
    );
    assert!(
        offset_after >= offset_before,
        "display_offset must not decrease during scroll-lock (before={offset_before}, after={offset_after})"
    );

    svc.kill(&aid).await.unwrap();
}
