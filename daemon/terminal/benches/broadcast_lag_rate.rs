//! Measures broadcast Lagged recovery at varying channel capacities.
//!
//! Two scenarios compare the fast replay path (large capacity, Lagged rare)
//! against the slow snapshot-fallback path (small capacity, frequent Lagged).
//! For offline analysis via `make bench-vt`; not a CI gate.
use bytes::Bytes;
use criterion::{Criterion, criterion_group, criterion_main};
use ozmux_terminal::testing::replay::{RecvOutcome, stream_wire_to_subscriber};
use ozmux_terminal::vt::WireMessage;
use tokio::runtime::Builder;

/// Builds a realistic mixed message sequence: binary deltas interleaved with
/// mode-change text frames and an optional oversize-error text frame.
fn make_mixed_messages(
    binary_count: u32,
    mode_every_n: u32,
    oversize_at_seq: Option<u32>,
) -> Vec<WireMessage> {
    let mut msgs: Vec<WireMessage> = Vec::new();
    let mut seq: u32 = 0;
    for i in 0..binary_count {
        if i > 0 && i % mode_every_n == 0 {
            msgs.push(WireMessage::Text(format!(
                "{{\"kind\":\"mode\",\"seq\":{seq},\"cursor_shape\":\"bar\"}}"
            )));
            seq += 1;
        }
        if Some(seq) == oversize_at_seq {
            msgs.push(WireMessage::Text(format!(
                "{{\"kind\":\"oversize_error\",\"seq\":{seq}}}"
            )));
            seq += 1;
        }
        let payload = Bytes::from(vec![0xCC; 256]);
        msgs.push(WireMessage::Binary {
            seq,
            encoded: payload,
        });
        seq += 1;
    }
    msgs
}

fn bench_replay_vs_snapshot(c: &mut Criterion) {
    let rt = Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .unwrap();

    let messages = make_mixed_messages(100, 20, Some(45));

    let mut group = c.benchmark_group("broadcast_lag_rate");

    group.bench_function("contiguous_ring_replay", |b| {
        b.to_async(&rt).iter(|| async {
            // Large broadcast capacity so Lagged is rare; recovery via replay
            // (the fast path even when Lagged occurs).
            let outcomes = stream_wire_to_subscriber(&messages, 64).await;
            let lagged = outcomes
                .iter()
                .filter(|o| matches!(o, RecvOutcome::Lagged { .. }))
                .count();
            std::hint::black_box(lagged);
        });
    });

    group.bench_function("small_cap_forces_snapshot", |b| {
        b.to_async(&rt).iter(|| async {
            // Tiny broadcast capacity forces frequent Lagged events, exercising
            // the slow fresh-snapshot fallback path.
            let outcomes = stream_wire_to_subscriber(&messages, 4).await;
            let lagged = outcomes
                .iter()
                .filter(|o| matches!(o, RecvOutcome::Lagged { .. }))
                .count();
            std::hint::black_box(lagged);
        });
    });

    group.finish();
}

criterion_group!(benches, bench_replay_vs_snapshot);
criterion_main!(benches);
