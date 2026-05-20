//! PR-E2a: validates that the new observability instrumentation fires
//! under realistic bridge scenarios. Uses metrics-util DebuggingRecorder
//! installed per-test via metrics::set_default_local_recorder, then
//! snapshots Counter and Histogram values by name + labels.

use bytes::Bytes;
use metrics_util::debugging::{DebugValue, DebuggingRecorder, Snapshotter};
use ozmux_multiplexer::{ActivityId, PaneId};
use ozmux_terminal::{SpawnOptions, TerminalService};
use std::collections::HashMap;
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

/// Creates a fresh `DebuggingRecorder` + `Snapshotter` pair. The caller
/// keeps the recorder alive on its own stack and installs it via
/// `metrics::set_default_local_recorder(&recorder)`. The recorder must
/// outlive the `_guard` returned by `set_default_local_recorder`, which
/// is why the install is done inline by the test rather than abstracted
/// into a helper that returns the guard by value.
fn new_recorder() -> (DebuggingRecorder, Snapshotter) {
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    (recorder, snapshotter)
}

/// Returns the counter value for `name` + `labels` (subset match), or None.
fn counter_value(
    snapshotter: &Snapshotter,
    name: &str,
    labels: &[(&str, &str)],
) -> Option<u64> {
    snapshotter
        .snapshot()
        .into_vec()
        .into_iter()
        .find_map(|(key, _unit, _desc, value)| {
            if key.key().name() != name {
                return None;
            }
            let key_labels: HashMap<&str, &str> =
                key.key().labels().map(|l| (l.key(), l.value())).collect();
            for (k, v) in labels {
                if key_labels.get(k) != Some(v) {
                    return None;
                }
            }
            match value {
                DebugValue::Counter(c) => Some(c),
                _ => None,
            }
        })
}

/// Returns the histogram sample count for `name` + `labels`, or None.
fn histogram_count(
    snapshotter: &Snapshotter,
    name: &str,
    labels: &[(&str, &str)],
) -> Option<usize> {
    snapshotter
        .snapshot()
        .into_vec()
        .into_iter()
        .find_map(|(key, _unit, _desc, value)| {
            if key.key().name() != name {
                return None;
            }
            let key_labels: HashMap<&str, &str> =
                key.key().labels().map(|l| (l.key(), l.value())).collect();
            for (k, v) in labels {
                if key_labels.get(k) != Some(v) {
                    return None;
                }
            }
            match value {
                DebugValue::Histogram(samples) => Some(samples.len()),
                _ => None,
            }
        })
}

#[tokio::test]
async fn install_recorder_helper_works() {
    let (recorder, snapshotter) = new_recorder();
    let _guard = metrics::set_default_local_recorder(&recorder);
    metrics::counter!("__test_smoke").increment(1);
    assert_eq!(counter_value(&snapshotter, "__test_smoke", &[]), Some(1));
}

#[tokio::test]
async fn emit_duration_recorded_on_initial_snapshot() {
    let (recorder, snapshotter) = new_recorder();
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
    let (recorder, snapshotter) = new_recorder();
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
    let (recorder, snapshotter) = new_recorder();
    let _guard = metrics::set_default_local_recorder(&recorder);

    let (svc, aid) = spawn_with_emit(80, 24).await;
    let chunk_tx = svc.vt_chunk_sender_for_test(&aid).await.unwrap();

    // First chunk -> initial snapshot. Second chunk -> at least one delta.
    chunk_tx.send(Bytes::from_static(b"first\n")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    chunk_tx.send(Bytes::from_static(b"second\n")).await.unwrap();
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
