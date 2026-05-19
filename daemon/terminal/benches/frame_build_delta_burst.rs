//! Measures p50/p95/p99 wall time of frame_builder::build_delta on the
//! canonical PTY tape (proxied via feed_pty_tape).
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

fn bench_frame_build(c: &mut Criterion) {
    let rt = Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .unwrap();

    let path = fixture_path("synthetic_scroll_burst");
    let tape = match rt.block_on(async { Tape::load(&path) }) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("frame_build_delta_burst: tape not loadable ({e}); skipping bench");
            return;
        }
    };

    c.bench_function("frame_build_delta_burst_synthetic", |b| {
        b.to_async(&rt).iter(|| async {
            let _ = feed_pty_tape(&tape, ReplayMode::Immediate).await;
        });
    });
}

criterion_group!(benches, bench_frame_build);
criterion_main!(benches);
