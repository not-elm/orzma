//! Measures end-to-end PTY byte → broadcast subscribe receive latency.
use criterion::{Criterion, criterion_group, criterion_main};
use ozmux_terminal::testing::replay::{ReplayMode, feed_pty_tape};
use ozmux_terminal::testing::tape::Tape;
use std::path::PathBuf;
use tokio::runtime::Builder;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/pty_tapes")
        .join(format!("{name}.tape"))
}

fn bench_bridge_to_subscribe(c: &mut Criterion) {
    let rt = Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .unwrap();

    let path = fixture_path("synthetic_scroll_burst");
    let tape = match rt.block_on(async { Tape::load(&path) }) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("bridge_to_subscribe: tape not loadable ({e}); skipping bench");
            return;
        }
    };

    c.bench_function("bridge_to_subscribe_synthetic", |b| {
        b.to_async(&rt).iter(|| async {
            let _ = feed_pty_tape(&tape, ReplayMode::Immediate).await;
        });
    });
}

criterion_group!(benches, bench_bridge_to_subscribe);
criterion_main!(benches);
