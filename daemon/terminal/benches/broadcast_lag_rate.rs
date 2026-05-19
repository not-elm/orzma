//! Measures broadcast Lagged events at varying capacities.
use criterion::{Criterion, criterion_group, criterion_main};
use ozmux_terminal::testing::replay::{stream_wire_to_subscriber, RecvOutcome};
use ozmux_terminal::testing::tape::Tape;
use ozmux_terminal::vt::WireMessage;
use std::path::PathBuf;
use tokio::runtime::Builder;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/pty_tapes")
        .join(format!("{name}.tape"))
}

fn bench_broadcast_lag_rate(c: &mut Criterion) {
    let rt = Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .unwrap();

    let path = fixture_path("synthetic_scroll_burst");
    let _tape = match rt.block_on(async { Tape::load(&path) }) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("broadcast_lag_rate: tape not loadable ({e}); skipping bench");
            return;
        }
    };

    // Empty message vec for now; future iteration can derive from tape.
    let messages: Vec<WireMessage> = Vec::new();

    c.bench_function("broadcast_lag_rate_cap16", |b| {
        b.to_async(&rt).iter(|| async {
            let outcomes = stream_wire_to_subscriber(&messages, 16).await;
            let lagged = outcomes.iter().filter(|o| matches!(o, RecvOutcome::Lagged { .. })).count();
            std::hint::black_box(lagged);
        });
    });
}

criterion_group!(benches, bench_broadcast_lag_rate);
criterion_main!(benches);
