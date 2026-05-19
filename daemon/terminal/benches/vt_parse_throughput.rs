//! VT parser throughput benchmark (stub — implementation added in a subsequent task).

use criterion::{criterion_group, criterion_main, Criterion};

fn vt_parse_throughput(_c: &mut Criterion) {}

criterion_group!(benches, vt_parse_throughput);
criterion_main!(benches);
