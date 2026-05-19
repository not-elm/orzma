//! Frame-builder delta burst benchmark (stub — implementation added in a subsequent task).

use criterion::{criterion_group, criterion_main, Criterion};

fn frame_build_delta_burst(_c: &mut Criterion) {}

criterion_group!(benches, frame_build_delta_burst);
criterion_main!(benches);
