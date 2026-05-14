//! Emits Phase 2A wire fixtures (binary MessagePack + JSON text).
//!
//! Usage:
//!   `cargo run -p ozmux_terminal --example emit_fixture` — emits snapshot_minimal only (Phase 1 compat)
//!   `cargo run -p ozmux_terminal --example emit_fixture -- --all` — emits all 4 fixtures + JSON

use ozmux_terminal::vt::{
    Color, Cursor, CursorShape, DirtyRow, FrameDelta, FrameSnapshot, ModeFrame, RenderFrame, Row,
    Run, SnapshotReason, encode,
};
use std::fs;
use std::path::Path;

fn main() {
    let dir = Path::new("daemon/terminal/tests/fixtures/wire_msgpack");
    fs::create_dir_all(dir).unwrap();

    let only_snapshot = std::env::args().nth(1).as_deref() != Some("--all");

    // 1) snapshot_minimal (Phase 1 baseline + modes field from Task 2)
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
    fs::write(dir.join("snapshot_minimal.bin"), encode(&snap).unwrap()).unwrap();
    fs::write(
        dir.join("snapshot_minimal.json"),
        serde_json::to_string_pretty(&snap).unwrap(),
    )
    .unwrap();

    if only_snapshot {
        return;
    }

    // 2) delta_minimal — wrapped in RenderFrame so the wire `kind` tag appears
    let delta_payload = FrameDelta {
        seq: 1,
        cursor: Cursor {
            x: 3,
            y: 0,
            shape: CursorShape::Block,
            visible: true,
        },
        dirty_rows: vec![DirtyRow {
            row: 0,
            runs: vec![Run {
                cols: 3,
                fg: Color::Default,
                bg: Color::Default,
                style: 0,
                text: "xyz".into(),
                hyperlink_id: None,
            }],
        }],
    };
    let delta_frame = RenderFrame::Delta(delta_payload);
    fs::write(dir.join("delta_minimal.bin"), encode(&delta_frame).unwrap()).unwrap();
    fs::write(
        dir.join("delta_minimal.json"),
        serde_json::to_string_pretty(&delta_frame).unwrap(),
    )
    .unwrap();

    // 3) mode_change — JSON text frame, stored as the JSON bytes (.bin = JSON encoding)
    let mode = ModeFrame::new(2, vec!["alt-screen".to_string()], vec![]);
    let mode_json = serde_json::to_string(&mode).unwrap();
    fs::write(dir.join("mode_change.bin"), &mode_json).unwrap();
    fs::write(
        dir.join("mode_change.json"),
        serde_json::to_string_pretty(&mode).unwrap(),
    )
    .unwrap();

    // 4) mode_change_mouse — JSON text frame announcing the renamed mouse modes.
    let mode_mouse = ModeFrame::new(
        3,
        vec!["mouse-vt200".to_string(), "mouse-btn-event".to_string()],
        vec![],
    );
    let mode_mouse_json = serde_json::to_string(&mode_mouse).unwrap();
    fs::write(dir.join("mode_change_mouse.bin"), &mode_mouse_json).unwrap();
    fs::write(
        dir.join("mode_change_mouse.json"),
        serde_json::to_string_pretty(&mode_mouse).unwrap(),
    )
    .unwrap();

    // 5) snapshot_modes_mouse — msgpack snapshot whose modes field exercises
    //    the renamed mouse mode strings end-to-end.
    let snap_mouse = FrameSnapshot {
        seq: 7,
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
        modes: vec!["mouse-any-event".to_string(), "mouse-sgr-1006".to_string()],
    };
    fs::write(
        dir.join("snapshot_modes_mouse.bin"),
        encode(&snap_mouse).unwrap(),
    )
    .unwrap();
    fs::write(
        dir.join("snapshot_modes_mouse.json"),
        serde_json::to_string_pretty(&snap_mouse).unwrap(),
    )
    .unwrap();

    // 6) hello — JSON text frame.
    //    NOTE: escape_caps mirrors http_server `vt_ws.rs::ESCAPE_CAPS`. Update
    //    both together when the wire capability list changes.
    let hello = serde_json::json!({
        "kind": "hello",
        "seq": 0,
        "cols": 80,
        "rows": 24,
        "cursor": { "x": 0, "y": 0, "shape": "block", "visible": true },
        "escape_caps": [
            "sgr", "cup", "ed", "el", "decset", "decrst", "alt-screen-1049", "bracketed-paste",
            "mouse-vt200", "mouse-btn-event", "mouse-any-event", "mouse-sgr-1006",
            "focus-events", "app-cursor-keys",
        ],
        "input_caps": ["text-utf8", "key-vt-encoded"],
    });
    fs::write(dir.join("hello.bin"), hello.to_string()).unwrap();
    fs::write(
        dir.join("hello.json"),
        serde_json::to_string_pretty(&hello).unwrap(),
    )
    .unwrap();

    eprintln!("wrote 6 fixtures to {}", dir.display());
}
