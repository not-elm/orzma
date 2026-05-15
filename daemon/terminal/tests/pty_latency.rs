//! Raw bytes path echo round-trip latency regression test.
//!
//! Phase 1 of the server-side VT bridge fans every PTY chunk out to a
//! best-effort `try_send` mpsc (cap = 128) that feeds the VT bridge task.
//! The raw path remains source of truth, so the bridge must NOT degrade
//! raw-path latency. This test pins that guarantee with a hard p99 ≤ 100ms
//! threshold for 100 small writes through `cat`.

use std::time::{Duration, Instant};

use ozmux_multiplexer::{ActivityId, PaneId};
use ozmux_terminal::{SpawnOptions, TerminalEvent, TerminalService};

#[tokio::test]
async fn raw_echo_p99_under_100ms() {
    let svc = TerminalService::default();
    let pane = PaneId::new();
    let activity = ActivityId::new();

    // `cat` echoes anything we write straight back as PTY output. We also
    // get the PTY's own line-discipline echo, but substring matching on a
    // unique per-iteration marker tolerates either source.
    svc.spawn(
        pane,
        activity.clone(),
        SpawnOptions {
            cols: 80,
            rows: 24,
            shell: "/bin/cat".to_string(),
            cwd: None,
            window_id: None,
            session_id: None,
        },
    )
    .await
    .expect("spawn");

    let (_snapshot, mut rx) = svc
        .snapshot_and_subscribe(&activity)
        .await
        .expect("subscribe");

    // Give cat a moment to settle so the first iteration isn't paying the
    // process-startup cost.
    tokio::time::sleep(Duration::from_millis(50)).await;

    const ITERS: usize = 100;
    let mut latencies = Vec::with_capacity(ITERS);
    for i in 0..ITERS {
        // Unique marker per iteration so stale bytes from earlier writes
        // cannot satisfy this iteration's wait.
        let marker = format!("ozmux-lat-{i:03}");
        let payload = format!("{marker}\n");

        let start = Instant::now();
        svc.write(&activity, payload.as_bytes())
            .await
            .expect("write");

        let mut got = Vec::<u8>::new();
        loop {
            let evt = tokio::time::timeout(Duration::from_secs(1), rx.recv())
                .await
                .expect("timeout waiting for echo")
                .expect("broadcast closed");
            match evt {
                TerminalEvent::Data { buffer } => {
                    got.extend_from_slice(&buffer);
                    if got.windows(marker.len()).any(|w| w == marker.as_bytes()) {
                        break;
                    }
                }
                TerminalEvent::Exit { .. } => panic!("cat exited mid-test"),
            }
        }
        latencies.push(start.elapsed());
    }

    latencies.sort();
    let p50 = latencies[ITERS / 2];
    let p99 = latencies[(ITERS * 99) / 100 - 1]; // index 98 of 100 sorted samples
    eprintln!("raw echo latency over {ITERS} iterations: p50={p50:?} p99={p99:?}");

    // Cleanup before assertion so a failing assert still releases the PTY.
    svc.kill(&activity).await.ok();

    assert!(
        p99 < Duration::from_millis(100),
        "raw echo p99 latency {p99:?} exceeds 100ms threshold (p50 {p50:?})"
    );
}
