//! Bridge-to-subscribe latency benchmark (stub — implementation added in a subsequent task).

use criterion::{criterion_group, criterion_main, Criterion};

fn bridge_to_subscribe(_c: &mut Criterion) {}

criterion_group!(benches, bridge_to_subscribe);
criterion_main!(benches);
