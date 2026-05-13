//! Run with: cargo run -p ozmux_terminal --example emit_fixture
//! Emits tests/fixtures/wire_msgpack/snapshot_minimal.bin

use std::path::Path;

use ozmux_terminal::vt::{Color, Cursor, CursorShape, FrameSnapshot, Row, Run, SnapshotReason};

fn main() {
    let snap = FrameSnapshot {
        seq: 0,
        cols: 3,
        rows: 1,
        cursor: Cursor {
            x: 0,
            y: 0,
            shape: CursorShape::Block,
            visible: true,
        },
        rows_data: vec![Row {
            runs: vec![Run {
                cols: 3,
                fg: Color::Default,
                bg: Color::Default,
                style: 0,
                text: "abc".into(),
                hyperlink_id: None,
            }],
        }],
        reason: SnapshotReason::Initial,
        modes: vec![],
    };
    let bytes = ozmux_terminal::vt::encode(&snap).expect("encode");
    let path = Path::new("daemon/terminal/tests/fixtures/wire_msgpack/snapshot_minimal.bin");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, &bytes).unwrap();
    eprintln!("wrote {} bytes to {}", bytes.len(), path.display());
}
