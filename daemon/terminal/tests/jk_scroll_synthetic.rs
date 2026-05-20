//! Synthetic NeoVim jk-scroll workload — drives the VT bridge with
//! a realistic stream of multi-row dirty chunks + user-input keystrokes,
//! then dumps the 4 PR-E2a metrics so we can determine which of
//! Hypothesis A/B/C dominates without a live NeoVim session.
//!
//! Run with:
//!   cargo test -p ozmux_terminal --features test-helpers \
//!     --test jk_scroll_synthetic -- --ignored --nocapture
//!
//! The test is `#[ignore]` so it does not run on normal `cargo test`.

use bytes::Bytes;
use metrics_util::CompositeKey;
use metrics_util::debugging::{DebugValue, DebuggingRecorder};
use ozmux_multiplexer::{ActivityId, PaneId};
use ozmux_terminal::{SpawnOptions, TerminalService};
use std::collections::HashMap;
use std::time::Duration;

type SnapshotRow = (
    CompositeKey,
    Option<metrics::Unit>,
    Option<metrics::SharedString>,
    DebugValue,
);

const CHUNKS: usize = 200;
const CHUNK_INTERVAL_MS: u64 = 15;

/// One "j press -> nvim 1-line scroll" payload: dirties row 24 (clear +
/// rewrite) and row 1 (status line), forcing ManyRows verdict.
fn nvim_scroll_chunk(seq: usize) -> Bytes {
    let payload = format!(
        "\x1b7\x1b[24;1H\x1b[K\x1b[24;1Hline {seq:04}: lorem ipsum dolor sit amet consectetur\x1b[1;1H-- INSERT --  {seq:04} \x1b8"
    );
    Bytes::from(payload)
}

/// Returns the counter value for `name` + `labels` (subset match), or None.
/// NOTE: `DebuggingRecorder::snapshot()` drains histogram samples on each
/// call, so the caller MUST take a single snapshot and reuse it via this
/// slice-based lookup rather than re-snapshotting per query.
fn counter_value(rows: &[SnapshotRow], name: &str, labels: &[(&str, &str)]) -> Option<u64> {
    rows.iter().find_map(|(key, _u, _d, v)| {
        if key.key().name() != name {
            return None;
        }
        let kl: HashMap<&str, &str> = key.key().labels().map(|l| (l.key(), l.value())).collect();
        for (k, val) in labels {
            if kl.get(k) != Some(val) {
                return None;
            }
        }
        match v {
            DebugValue::Counter(c) => Some(*c),
            _ => None,
        }
    })
}

/// Returns histogram sample values (in seconds) for `name` + `labels`, or None.
fn histogram_samples(
    rows: &[SnapshotRow],
    name: &str,
    labels: &[(&str, &str)],
) -> Option<Vec<f64>> {
    rows.iter().find_map(|(key, _u, _d, v)| {
        if key.key().name() != name {
            return None;
        }
        let kl: HashMap<&str, &str> = key.key().labels().map(|l| (l.key(), l.value())).collect();
        for (k, val) in labels {
            if kl.get(k) != Some(val) {
                return None;
            }
        }
        match v {
            DebugValue::Histogram(samples) => {
                Some(samples.iter().map(|s| s.into_inner()).collect())
            }
            _ => None,
        }
    })
}

/// Returns (p50, p99) from a histogram of samples, in milliseconds.
fn p50_p99_ms(mut samples: Vec<f64>) -> (f64, f64) {
    if samples.is_empty() {
        return (0.0, 0.0);
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p50 = samples[samples.len() / 2] * 1000.0;
    let p99_idx = ((samples.len() as f64) * 0.99) as usize;
    let p99 = samples[p99_idx.min(samples.len() - 1)] * 1000.0;
    (p50, p99)
}

#[tokio::test]
#[ignore = "synthetic load test - run explicitly with --ignored"]
async fn synthetic_jk_scroll_dumps_pr_e2a_metrics() {
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    let _guard = metrics::set_default_local_recorder(&recorder);

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
    .expect("spawn ok");

    // Allow bootstrap initial emit to settle.
    tokio::time::sleep(Duration::from_millis(150)).await;

    let chunk_tx = svc
        .vt_chunk_sender_for_test(&aid)
        .await
        .expect("test chunk sender");

    // Drive 200 chunks of realistic nvim scroll output, interleaved
    // with svc.write() to simulate user keystrokes (sets pending_user_input).
    for i in 0..CHUNKS {
        // Simulate user pressing 'j' - sets pending_user_input on VtState.
        let _ = svc.write(&aid, b"j").await;
        // Send the nvim scroll-redraw payload.
        let _ = chunk_tx.send(nvim_scroll_chunk(i)).await;
        // Pace at ~67 cps (typical keyboard autorepeat).
        tokio::time::sleep(Duration::from_millis(CHUNK_INTERVAL_MS)).await;
    }

    // Let the bridge drain.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Single snapshot — DebuggingRecorder drains histograms on every
    // .snapshot() call, so all subsequent lookups must be served from
    // this `rows` vector rather than re-snapshotting.
    let rows: Vec<SnapshotRow> = snapshotter.snapshot().into_vec();

    println!("\n========================================");
    println!("PR-E2a Synthetic jk-scroll metrics dump");
    println!("========================================");
    println!("Workload: {CHUNKS} chunks @ {CHUNK_INTERVAL_MS}ms interval");
    println!();

    for kind in ["snapshot", "delta"] {
        if let Some(samples) = histogram_samples(
            &rows,
            "ozmux_terminal_emit_duration_seconds",
            &[("kind", kind)],
        ) {
            let count = samples.len();
            let (p50, p99) = p50_p99_ms(samples);
            println!(
                "emit_duration_seconds[kind={kind:8}]: count={count:4}  p50={p50:6.2}ms  p99={p99:6.2}ms"
            );
        } else {
            println!("emit_duration_seconds[kind={kind:8}]: (no samples)");
        }
    }

    if let Some(samples) =
        histogram_samples(&rows, "ozmux_terminal_coalesce_wait_seconds", &[])
    {
        let count = samples.len();
        let lt_3 = samples.iter().filter(|&&s| s < 0.003).count();
        let lt_6 = samples.iter().filter(|&&s| s >= 0.003 && s < 0.006).count();
        let lt_12 = samples.iter().filter(|&&s| s >= 0.006 && s < 0.012).count();
        let lt_25 = samples
            .iter()
            .filter(|&&s| s >= 0.012 && s < 0.025)
            .count();
        let ge_25 = samples.iter().filter(|&&s| s >= 0.025).count();
        let (p50, p99) = p50_p99_ms(samples);
        println!(
            "coalesce_wait_seconds:                count={count:4}  p50={p50:6.2}ms  p99={p99:6.2}ms"
        );
        println!(
            "  buckets: <3ms={lt_3:4}  3-6ms={lt_6:4}  6-12ms={lt_12:4}  12-25ms={lt_25:4}  >=25ms={ge_25:4}"
        );
    } else {
        println!("coalesce_wait_seconds: (no samples)");
    }

    for reason in ["initial", "resize", "threshold"] {
        let v = counter_value(
            &rows,
            "ozmux_terminal_snapshot_total",
            &[("reason", reason)],
        )
        .unwrap_or(0);
        println!("snapshot_total[reason={reason:9}]:     {v}");
    }

    let drops =
        counter_value(&rows, "ozmux_terminal_pty_chunk_drops_total", &[]).unwrap_or(0);
    println!("pty_chunk_drops_total:                  {drops}");

    let snap_total = counter_value(
        &rows,
        "ozmux_frames_emit_total",
        &[("kind", "snapshot")],
    )
    .unwrap_or(0);
    let delta_total = counter_value(
        &rows,
        "ozmux_frames_emit_total",
        &[("kind", "delta")],
    )
    .unwrap_or(0);
    println!();
    println!("frames_emit_total[kind=snapshot]:       {snap_total}");
    println!("frames_emit_total[kind=delta]:          {delta_total}");

    println!("\n========================================");
    println!("Hypothesis assessment");
    println!("========================================");
    let drops_ticked = drops > 0;
    let threshold_ticked = counter_value(
        &rows,
        "ozmux_terminal_snapshot_total",
        &[("reason", "threshold")],
    )
    .unwrap_or(0)
        > 0;

    let coalesce_samples =
        histogram_samples(&rows, "ozmux_terminal_coalesce_wait_seconds", &[])
            .unwrap_or_default();
    let coalesce_count = coalesce_samples.len();
    let coalesce_p99_ms = if coalesce_samples.is_empty() {
        0.0
    } else {
        p50_p99_ms(coalesce_samples).1
    };
    let emit_samples = histogram_samples(
        &rows,
        "ozmux_terminal_emit_duration_seconds",
        &[("kind", "delta")],
    )
    .unwrap_or_default();
    let emit_delta_p99_ms = if emit_samples.is_empty() {
        0.0
    } else {
        p50_p99_ms(emit_samples).1
    };

    println!(
        "Hypothesis A (coalesce wait dominates): coalesce_p99={coalesce_p99_ms:.2}ms, emit_delta_p99={emit_delta_p99_ms:.2}ms"
    );
    if coalesce_p99_ms >= 10.0 && emit_delta_p99_ms < 5.0 {
        println!("  -> CONFIRMED");
    } else if coalesce_count > 0 {
        println!("  -> not confirmed (coalesce_p99 < 10ms OR emit_p99 >= 5ms)");
    } else {
        println!("  -> no coalesce_wait samples (bridge never took the deadline arm)");
    }
    println!(
        "Hypothesis B (snapshot threshold): snapshot_total[reason=threshold] ticked: {threshold_ticked}"
    );
    if threshold_ticked {
        println!("  -> CONFIRMED");
    } else {
        println!("  -> not confirmed");
    }
    println!(
        "Hypothesis C (PTY chunk drops): pty_chunk_drops_total ticked: {drops_ticked} (drops={drops})"
    );
    if drops_ticked {
        println!("  -> CONFIRMED");
    } else {
        println!("  -> not confirmed");
    }

    svc.kill(&aid).await.expect("kill ok");
}
