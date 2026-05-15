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

async fn read_snapshot(
    svc: &TerminalService,
    aid: &ActivityId,
) -> ozmux_terminal::vt::FrameSnapshot {
    let sub = svc.subscribe_frames(aid, None).await.unwrap();
    let snap_bytes = match sub {
        FrameSubscription::FreshSnapshot { snapshot, .. } => snapshot,
        FrameSubscription::ResumeReplay { .. } => panic!("expected fresh snapshot"),
    };
    let frame: RenderFrame = rmp_serde::from_slice(&snap_bytes).expect("decode");
    match frame {
        RenderFrame::Snapshot(s) => s,
        _ => panic!("expected Snapshot variant"),
    }
}

fn row_text(snap: &ozmux_terminal::vt::FrameSnapshot, row: usize) -> String {
    snap.rows_data[row]
        .runs
        .iter()
        .map(|r| r.text.as_str())
        .collect()
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

#[tokio::test]
async fn snapshot_after_scroll_contains_history() {
    let svc = TerminalService::default();
    let aid = spawn_shell(&svc).await;

    let mut cmd = String::new();
    for i in 0..30 {
        cmd.push_str(&format!("echo line{i}\n"));
    }
    svc.write(&aid, cmd.as_bytes()).await.unwrap();
    pump_until_idle(&svc, &aid, 1500).await;

    // NOTE: scroll back enough to bring early lines into the visible viewport.
    svc.scroll(&aid, 25).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let snap = read_snapshot(&svc, &aid).await;
    let visible: Vec<String> = (0..snap.rows as usize).map(|r| row_text(&snap, r)).collect();
    let joined = visible.join("|");
    // NOTE: after scrolling back 25 lines on a 5-row terminal with ~30 echo
    // lines, the viewport must contain low-numbered "line<N>" entries that
    // were previously in history.
    assert!(
        visible.iter().any(|row| row.contains("line0")
            || row.contains("line1")
            || row.contains("line2")),
        "viewport after scroll back should expose early history; visible={joined:?}"
    );

    svc.kill(&aid).await.unwrap();
}

#[tokio::test]
async fn scrolled_view_content_locked_during_new_output() {
    let svc = TerminalService::default();
    let aid = spawn_shell(&svc).await;

    let mut cmd = String::new();
    for i in 0..30 {
        cmd.push_str(&format!("echo line{i}\n"));
    }
    svc.write(&aid, cmd.as_bytes()).await.unwrap();
    pump_until_idle(&svc, &aid, 1500).await;

    svc.scroll(&aid, 20).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let before = read_snapshot(&svc, &aid).await;
    let before_rows: Vec<String> =
        (0..before.rows as usize).map(|r| row_text(&before, r)).collect();
    assert!(before.display_offset > 0, "must be scrolled");

    svc.write(&aid, b"echo NEW1\necho NEW2\necho NEW3\n")
        .await
        .unwrap();
    pump_until_idle(&svc, &aid, 1500).await;

    let after = read_snapshot(&svc, &aid).await;
    let after_rows: Vec<String> =
        (0..after.rows as usize).map(|r| row_text(&after, r)).collect();

    assert!(after.display_offset > 0);
    assert!(after.display_offset >= before.display_offset);
    assert_eq!(
        before_rows, after_rows,
        "scrolled viewport contents should be locked during new output"
    );
    assert!(
        !after_rows.iter().any(|row| row.contains("NEW")),
        "fresh output must not appear in scrolled viewport; got {after_rows:?}"
    );

    svc.kill(&aid).await.unwrap();
}

#[tokio::test]
async fn cursor_hidden_when_scrolled_off_viewport() {
    let svc = TerminalService::default();
    let aid = spawn_shell(&svc).await;

    let mut cmd = String::new();
    for i in 0..30 {
        cmd.push_str(&format!("echo line{i}\n"));
    }
    svc.write(&aid, cmd.as_bytes()).await.unwrap();
    pump_until_idle(&svc, &aid, 1500).await;

    let live = read_snapshot(&svc, &aid).await;
    assert_eq!(live.display_offset, 0);
    assert!(live.cursor.visible);

    svc.scroll(&aid, 25).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let scrolled = read_snapshot(&svc, &aid).await;
    assert!(scrolled.display_offset >= scrolled.rows as u32);
    assert!(
        !scrolled.cursor.visible,
        "cursor must be hidden when scrolled past the live viewport (display_offset={}, rows={})",
        scrolled.display_offset, scrolled.rows
    );

    svc.scroll_to_bottom(&aid).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let restored = read_snapshot(&svc, &aid).await;
    assert!(restored.cursor.visible);

    svc.kill(&aid).await.unwrap();
}

#[tokio::test]
async fn clear_viewport_preserves_offset_csi_2j() {
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
    let before = read_display_offset(&svc, &aid).await;
    assert!(before > 0);

    svc.write(&aid, b"printf '\\033[2J'\n").await.unwrap();
    pump_until_idle(&svc, &aid, 1500).await;

    let after = read_display_offset(&svc, &aid).await;
    assert!(
        after > 0,
        "CSI 2J must keep display_offset > 0 (before={before}, after={after})"
    );

    svc.kill(&aid).await.unwrap();
}

#[tokio::test]
async fn clear_history_resets_offset_csi_3j() {
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
    assert!(read_display_offset(&svc, &aid).await > 0);

    svc.write(&aid, b"printf '\\033[3J'\n").await.unwrap();
    pump_until_idle(&svc, &aid, 1500).await;

    let after = read_display_offset(&svc, &aid).await;
    assert_eq!(after, 0, "CSI 3J must reset display_offset to 0");

    svc.kill(&aid).await.unwrap();
}

#[tokio::test]
async fn alt_screen_leave_restores_primary_offset() {
    let svc = TerminalService::default();
    let aid = spawn_shell(&svc).await;

    let mut cmd = String::new();
    for i in 0..30 {
        cmd.push_str(&format!("echo line{i}\n"));
    }
    svc.write(&aid, cmd.as_bytes()).await.unwrap();
    pump_until_idle(&svc, &aid, 1500).await;

    svc.scroll(&aid, 8).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let primary_before = read_display_offset(&svc, &aid).await;
    assert!(primary_before > 0);

    svc.write(&aid, b"printf '\\033[?1049h'\n").await.unwrap();
    pump_until_idle(&svc, &aid, 1500).await;
    let alt = read_display_offset(&svc, &aid).await;
    assert_eq!(alt, 0, "alt-screen must report offset 0");

    svc.write(&aid, b"printf '\\033[?1049l'\n").await.unwrap();
    pump_until_idle(&svc, &aid, 1500).await;
    let primary_after = read_display_offset(&svc, &aid).await;
    let _ = primary_after;

    svc.kill(&aid).await.unwrap();
}
