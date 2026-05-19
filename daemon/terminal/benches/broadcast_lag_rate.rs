//! Broadcast lag rate benchmark (stub — implementation added in a subsequent task).

use criterion::{criterion_group, criterion_main, Criterion};

fn broadcast_lag_rate(_c: &mut Criterion) {}

criterion_group!(benches, broadcast_lag_rate);
criterion_main!(benches);
