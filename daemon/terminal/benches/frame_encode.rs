//! Encode latency benchmarks for FrameSnapshot/FrameDelta.
//!
//! Phase 1 PoC: verify the MessagePack encoder can produce frames within
//! the targets needed by the Phase 2 coalescer.
//!
//! Targets:
//! - snapshot 80x24:   < 100µs (sets the floor for resync/lagged paths)
//! - delta 4 rows:     <  20µs (typical hot-path payload size)

use criterion::{Criterion, black_box, criterion_group, criterion_main};

use ozmux_terminal::vt::{
    Color, Cursor, CursorShape, DirtyRow, FrameDelta, FrameSnapshot, Row, Run, SnapshotReason,
};

fn make_snapshot_80x24() -> FrameSnapshot {
    let row = Row {
        runs: vec![Run {
            cols: 80,
            fg: Color::Default,
            bg: Color::Default,
            style: 0,
            text: "abcdefghij".repeat(8), // 80 chars
            hyperlink_id: None,
        }],
    };
    FrameSnapshot {
        seq: 0,
        cols: 80,
        rows: 24,
        cursor: Cursor {
            x: 0,
            y: 0,
            shape: CursorShape::Block,
            visible: true,
        },
        rows_data: vec![row; 24],
        reason: SnapshotReason::Initial,
        modes: vec![],
    }
}

fn make_delta_4rows() -> FrameDelta {
    let run = Run {
        cols: 80,
        fg: Color::Default,
        bg: Color::Default,
        style: 0,
        text: "x".repeat(80),
        hyperlink_id: None,
    };
    FrameDelta {
        seq: 0,
        dirty_rows: (0..4u16)
            .map(|row| DirtyRow {
                row,
                runs: vec![run.clone()],
            })
            .collect(),
    }
}

fn bench_snapshot_encode(c: &mut Criterion) {
    let snap = make_snapshot_80x24();
    c.bench_function("snapshot_80x24_encode", |b| {
        b.iter(|| {
            let bytes = ozmux_terminal::vt::encode(black_box(&snap)).unwrap();
            black_box(bytes);
        });
    });
}

fn bench_delta_encode(c: &mut Criterion) {
    let delta = make_delta_4rows();
    c.bench_function("delta_4rows_encode", |b| {
        b.iter(|| {
            let bytes = ozmux_terminal::vt::encode(black_box(&delta)).unwrap();
            black_box(bytes);
        });
    });
}

criterion_group!(benches, bench_snapshot_encode, bench_delta_encode);
criterion_main!(benches);
