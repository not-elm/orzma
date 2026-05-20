//! Validates that the observability instrumentation fires under realistic
//! bridge scenarios. Uses metrics-util DebuggingRecorder installed per-test
//! via metrics::set_default_local_recorder, then snapshots Counter and
//! Histogram values by name + labels.

use bytes::Bytes;
use ozmux_multiplexer::{ActivityId, PaneId};
use ozmux_terminal::service::test_helpers::{
    counter_value, histogram_count, new_debugging_recorder,
};
use ozmux_terminal::{SpawnOptions, TerminalService};
use std::time::Duration;

/// Spawns a TerminalService + bootstrap activity at the given dimensions
/// and waits ~150 ms for the bridge to come up and emit its initial
/// snapshot. Mirrors the `spawn_terminal` helper used in `vt_emit.rs`.
async fn spawn_with_emit(cols: u16, rows: u16) -> (TerminalService, ActivityId) {
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
    // Give the bridge time to run the first emit (Initial snapshot).
    tokio::time::sleep(Duration::from_millis(150)).await;
    (svc, aid)
}

#[tokio::test]
async fn install_recorder_helper_works() {
    let (recorder, snapshotter) = new_debugging_recorder();
    let _guard = metrics::set_default_local_recorder(&recorder);
    metrics::counter!("__test_smoke").increment(1);
    assert_eq!(counter_value(&snapshotter, "__test_smoke", &[]), Some(1));
}

#[tokio::test]
async fn emit_duration_recorded_on_initial_snapshot() {
    let (recorder, snapshotter) = new_debugging_recorder();
    let _guard = metrics::set_default_local_recorder(&recorder);

    let (svc, aid) = spawn_with_emit(80, 24).await;
    // Drive any chunk to guarantee the bridge has gone through at least
    // one emit_now call. The initial-emit path records a snapshot sample.
    let chunk_tx = svc.vt_chunk_sender_for_test(&aid).await.unwrap();
    chunk_tx.send(Bytes::from_static(b"hello\n")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;

    let n = histogram_count(
        &snapshotter,
        "ozmux_terminal_emit_duration_seconds",
        &[("kind", "snapshot")],
    );
    assert!(
        n.unwrap_or(0) >= 1,
        "expected >= 1 snapshot emit_duration sample, got {n:?}"
    );
    svc.kill(&aid).await.unwrap();
}

#[tokio::test]
async fn coalesce_wait_recorded_on_deadline_emit() {
    let (recorder, snapshotter) = new_debugging_recorder();
    let _guard = metrics::set_default_local_recorder(&recorder);

    let (svc, aid) = spawn_with_emit(80, 24).await;
    let chunk_tx = svc.vt_chunk_sender_for_test(&aid).await.unwrap();

    // Drive an initial chunk so first_emit is false (immediate flush
    // path on bootstrap consumes the chunk without recording into
    // coalesce_wait — that arm only fires from wait_deadline).
    chunk_tx.send(Bytes::from_static(b"x\n")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Multi-row damage delivered via the test chunk sender: bypasses
    // svc.write so pending_user_input stays false. Verdict is ManyRows,
    // immediate-flush is false, so the coalescer arms and the deadline
    // arm fires after at most MAX_CAP (~12 ms).
    let many: Vec<u8> = (0..5)
        .flat_map(|i| format!("row {i:02}\r\n").into_bytes())
        .collect();
    chunk_tx.send(Bytes::from(many)).await.unwrap();
    // Wait well past MAX_CAP so the deadline definitely fires.
    tokio::time::sleep(Duration::from_millis(80)).await;

    let n = histogram_count(&snapshotter, "ozmux_terminal_coalesce_wait_seconds", &[]);
    assert!(
        n.unwrap_or(0) >= 1,
        "expected >= 1 coalesce_wait sample after a deadline-fired emit, got {n:?}"
    );
    svc.kill(&aid).await.unwrap();
}

#[tokio::test]
async fn emit_duration_recorded_on_delta_emit() {
    let (recorder, snapshotter) = new_debugging_recorder();
    let _guard = metrics::set_default_local_recorder(&recorder);

    let (svc, aid) = spawn_with_emit(80, 24).await;
    let chunk_tx = svc.vt_chunk_sender_for_test(&aid).await.unwrap();

    // First chunk -> initial snapshot. Second chunk -> at least one delta.
    chunk_tx.send(Bytes::from_static(b"first\n")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    chunk_tx
        .send(Bytes::from_static(b"second\n"))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;

    let n = histogram_count(
        &snapshotter,
        "ozmux_terminal_emit_duration_seconds",
        &[("kind", "delta")],
    );
    assert!(
        n.unwrap_or(0) >= 1,
        "expected >= 1 delta emit_duration sample, got {n:?}"
    );
    svc.kill(&aid).await.unwrap();
}

#[tokio::test]
async fn snapshot_total_by_reason_initial() {
    let (recorder, snapshotter) = new_debugging_recorder();
    let _guard = metrics::set_default_local_recorder(&recorder);

    let (svc, aid) = spawn_with_emit(80, 24).await;
    let chunk_tx = svc.vt_chunk_sender_for_test(&aid).await.unwrap();
    chunk_tx.send(Bytes::from_static(b"x\n")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;

    assert_eq!(
        counter_value(
            &snapshotter,
            "ozmux_terminal_snapshot_total",
            &[("reason", "initial")],
        ),
        Some(1),
    );
    svc.kill(&aid).await.unwrap();
}

#[tokio::test]
async fn snapshot_total_by_reason_resize() {
    let (recorder, snapshotter) = new_debugging_recorder();
    let _guard = metrics::set_default_local_recorder(&recorder);

    let (svc, aid) = spawn_with_emit(80, 24).await;
    let chunk_tx = svc.vt_chunk_sender_for_test(&aid).await.unwrap();
    chunk_tx.send(Bytes::from_static(b"x\n")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Resize: the bridge marks pending_emit_reason = Resize and emits
    // a Snapshot frame the next time emit_now runs.
    svc.resize(&aid, 80, 30).await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;

    let v = counter_value(
        &snapshotter,
        "ozmux_terminal_snapshot_total",
        &[("reason", "resize")],
    );
    assert!(
        v.unwrap_or(0) >= 1,
        "expected >= 1 snapshot with reason=resize, got {v:?}"
    );
    svc.kill(&aid).await.unwrap();
}

#[tokio::test]
async fn snapshot_total_sum_equals_frames_emit_total_snapshot_in_band() {
    let (recorder, snapshotter) = new_debugging_recorder();
    let _guard = metrics::set_default_local_recorder(&recorder);

    let (svc, aid) = spawn_with_emit(80, 24).await;
    let chunk_tx = svc.vt_chunk_sender_for_test(&aid).await.unwrap();

    // initial
    chunk_tx.send(Bytes::from_static(b"x\n")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // resize
    svc.resize(&aid, 80, 30).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // threshold: dirty >=85 % of 30 rows (>= 26 rows).
    let mut buf = Vec::new();
    for row in 1..=27 {
        buf.extend_from_slice(format!("\x1b[{row};1H").as_bytes());
        buf.push(b'X');
    }
    chunk_tx.send(Bytes::from(buf)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    let initial = counter_value(
        &snapshotter,
        "ozmux_terminal_snapshot_total",
        &[("reason", "initial")],
    )
    .unwrap_or(0);
    let resize = counter_value(
        &snapshotter,
        "ozmux_terminal_snapshot_total",
        &[("reason", "resize")],
    )
    .unwrap_or(0);
    let threshold = counter_value(
        &snapshotter,
        "ozmux_terminal_snapshot_total",
        &[("reason", "threshold")],
    )
    .unwrap_or(0);
    let total = counter_value(
        &snapshotter,
        "ozmux_frames_emit_total",
        &[("kind", "snapshot")],
    )
    .unwrap_or(0);
    assert_eq!(
        initial + resize + threshold,
        total,
        "in-band invariant violated: initial={initial} + resize={resize} + threshold={threshold} != frames_emit_total snapshot={total}",
    );
    svc.kill(&aid).await.unwrap();
}

#[tokio::test]
async fn subscribe_triggered_snapshot_does_not_tick_snapshot_total() {
    let (recorder, snapshotter) = new_debugging_recorder();
    let _guard = metrics::set_default_local_recorder(&recorder);

    let (svc, aid) = spawn_with_emit(80, 24).await;
    let chunk_tx = svc.vt_chunk_sender_for_test(&aid).await.unwrap();
    chunk_tx.send(Bytes::from_static(b"x\n")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;

    let initial_before = counter_value(
        &snapshotter,
        "ozmux_terminal_snapshot_total",
        &[("reason", "initial")],
    )
    .unwrap_or(0);
    assert_eq!(initial_before, 1);

    // Subscribe with last_seq=None -> FreshSnapshot path. The snapshot
    // is built outside emit_now and must NOT increment snapshot_total.
    let _sub = svc.subscribe_frames(&aid, None).await.unwrap();

    let initial_after = counter_value(
        &snapshotter,
        "ozmux_terminal_snapshot_total",
        &[("reason", "initial")],
    )
    .unwrap_or(0);
    assert_eq!(
        initial_after, 1,
        "subscribe-triggered FreshSnapshot must NOT tick snapshot_total"
    );
    svc.kill(&aid).await.unwrap();
}

#[tokio::test]
async fn snapshot_total_by_reason_threshold() {
    let (recorder, snapshotter) = new_debugging_recorder();
    let _guard = metrics::set_default_local_recorder(&recorder);

    let (svc, aid) = spawn_with_emit(80, 24).await;
    let chunk_tx = svc.vt_chunk_sender_for_test(&aid).await.unwrap();

    // Initial snapshot emit.
    chunk_tx.send(Bytes::from_static(b"x\n")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Now produce damage spanning >= 85 % of 24 rows. The decide_frame_kind
    // threshold check (rows.len() * 20 >= total_rows * 17) promotes the
    // delta to a Snapshot with reason=Deadline/Immediate, which maps to
    // the snapshot_total_threshold counter. Use position-jump sequences
    // so 22 distinct viewport rows are dirtied within a single chunk.
    let mut buf = Vec::new();
    for row in 1..=22 {
        // CSI <row>;1H -> move cursor; followed by a single byte to
        // dirty that row.
        buf.extend_from_slice(format!("\x1b[{row};1H").as_bytes());
        buf.push(b'X');
    }
    chunk_tx.send(Bytes::from(buf)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(80)).await;

    let v = counter_value(
        &snapshotter,
        "ozmux_terminal_snapshot_total",
        &[("reason", "threshold")],
    );
    assert!(
        v.unwrap_or(0) >= 1,
        "expected >= 1 snapshot with reason=threshold, got {v:?}"
    );
    svc.kill(&aid).await.unwrap();
}

#[tokio::test]
async fn pr_e2b_many_rows_with_user_input_skips_coalesce_wait() {
    let (recorder, snapshotter) = new_debugging_recorder();
    let _guard = metrics::set_default_local_recorder(&recorder);

    let (svc, aid) = spawn_with_emit(80, 24).await;
    let chunk_tx = svc.vt_chunk_sender_for_test(&aid).await.unwrap();

    // 20 chunks of "2-row nvim scroll" payload + user-input keystroke.
    // Inter-chunk spacing 20 ms > IDLE = 3 ms so each chunk is
    // independent — pre-PR-E2b would yield ~20 coalesce_wait samples.
    for i in 0..20u16 {
        let _ = svc.write(&aid, b"j").await;
        let payload = format!("\x1b7\x1b[24;1H\x1b[Kline{i:03}\x1b[1;1Hstatus{i:03}\x1b8");
        let _ = chunk_tx.send(Bytes::from(payload)).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_millis(100)).await;

    let n = histogram_count(&snapshotter, "ozmux_terminal_coalesce_wait_seconds", &[]).unwrap_or(0);

    // Before the immediate-flush fix, every chunk was debounced (~20 samples).
    // After the fix, chunks take the immediate-flush path and at most 1-2
    // stragglers remain due to boundary effects.
    assert!(
        n <= 2,
        "expected <= 2 coalesce_wait samples after immediate-flush \
         (chunks should bypass debounce), got {n}"
    );
    svc.kill(&aid).await.unwrap();
}
