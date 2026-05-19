//! Emits all wire-protocol fixtures (binary msgpack + JSON text) used by the
//! wire-contract regression tests.
//!
//! Usage:
//!   `cargo run -p ozmux_terminal --example emit_fixture -- --all`
//!
//! Writes/overwrites every `.bin` and `.json` under
//! `daemon/terminal/tests/fixtures/wire_msgpack/`. The `.diag.txt` golden
//! files are NOT touched here — regenerate those via
//! `make test-wire-goldens` (which uses `tools/bin-to-diag.sh`).
use ozmux_terminal::vt::{
    Color, Cursor, CursorShape, DirtyRow, FrameDelta, FrameSnapshot, Hyperlink, HyperlinkUri,
    HyperlinkWireId, ModeFrame, RenderFrame, Row, Run, SnapshotReason, encode,
};
use std::fs;
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) != Some("--all") {
        eprintln!("usage: cargo run -p ozmux_terminal --example emit_fixture -- --all");
        std::process::exit(2);
    }

    let dir = Path::new("daemon/terminal/tests/fixtures/wire_msgpack");
    fs::create_dir_all(dir).expect("create fixture dir");

    write_snapshot(dir, "snapshot_minimal", &snapshot_minimal());
    write_snapshot(dir, "snapshot_cursor_blink", &snapshot_cursor_blink());
    write_snapshot(dir, "snapshot_modes_mouse", &snapshot_modes_mouse());
    write_snapshot(dir, "snapshot_with_hyperlinks", &snapshot_with_hyperlinks());

    write_delta(dir, "delta_minimal", &delta_minimal());
    write_delta(dir, "delta_cursor_shape", &delta_cursor_shape());
    write_delta(dir, "delta_with_hyperlinks", &delta_with_hyperlinks());

    write_text(dir, "hello", &hello_json());
    write_text(
        dir,
        "mode_change",
        &mode_change(2, vec!["alt-screen".into()], vec![]),
    );
    write_text(
        dir,
        "mode_change_mouse",
        &mode_change(3, vec!["mouse-sgr-1006".into()], vec!["alt-screen".into()]),
    );

    println!("emit_fixture: 4 snapshots + 3 deltas + 3 text fixtures written");
}

fn write_snapshot(dir: &Path, name: &str, snap: &FrameSnapshot) {
    let frame = RenderFrame::Snapshot(snap.clone());
    let bin = encode(&frame).expect("encode snapshot");
    fs::write(dir.join(format!("{name}.bin")), &bin).expect("write .bin");
    let json = serde_json::to_string_pretty(&frame).expect("json pretty");
    fs::write(dir.join(format!("{name}.json")), json).expect("write .json");
}

fn snapshot_minimal() -> FrameSnapshot {
    FrameSnapshot {
        seq: 0,
        cols: 3,
        rows: 1,
        cursor: Cursor {
            x: 0,
            y: 0,
            shape: CursorShape::Block,
            blinking: false,
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
        hyperlinks: vec![],
        display_offset: 0,
        history_size: 0,
        produced_at_us: None,
    }
}

fn snapshot_cursor_blink() -> FrameSnapshot {
    let mut s = snapshot_minimal();
    s.seq = 3;
    s.cursor.blinking = true;
    s
}

fn snapshot_modes_mouse() -> FrameSnapshot {
    let mut s = snapshot_minimal();
    s.seq = 7;
    s.modes = vec!["mouse-btn-event".into(), "mouse-sgr-1006".into()];
    s
}

fn snapshot_with_hyperlinks() -> FrameSnapshot {
    let mut s = snapshot_minimal();
    s.seq = 1;
    s.rows_data[0].runs[0].hyperlink_id = Some(HyperlinkWireId(1));
    s.hyperlinks = vec![Hyperlink {
        id: HyperlinkWireId(1),
        uri: HyperlinkUri::new("https://example.com/"),
    }];
    s
}

fn write_delta(dir: &Path, name: &str, delta: &FrameDelta) {
    let frame = RenderFrame::Delta(delta.clone());
    let bin = encode(&frame).expect("encode delta");
    fs::write(dir.join(format!("{name}.bin")), &bin).expect("write .bin");
    let json = serde_json::to_string_pretty(&frame).expect("json pretty");
    fs::write(dir.join(format!("{name}.json")), json).expect("write .json");
}

fn delta_minimal() -> FrameDelta {
    FrameDelta {
        seq: 1,
        cursor: Cursor {
            x: 0,
            y: 0,
            shape: CursorShape::Block,
            blinking: false,
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
        hyperlinks: vec![],
        display_offset: 0,
        produced_at_us: None,
    }
}

fn delta_cursor_shape() -> FrameDelta {
    let mut d = delta_minimal();
    d.seq = 4;
    d.cursor = Cursor {
        x: 1,
        y: 0,
        shape: CursorShape::Bar,
        blinking: false,
        visible: true,
    };
    d
}

fn delta_with_hyperlinks() -> FrameDelta {
    let mut d = delta_minimal();
    d.seq = 2;
    d.dirty_rows[0].runs[0].hyperlink_id = Some(HyperlinkWireId(1));
    d.hyperlinks = vec![Hyperlink {
        id: HyperlinkWireId(1),
        uri: HyperlinkUri::new("https://example.com/"),
    }];
    d
}

fn write_text(dir: &Path, name: &str, value: &serde_json::Value) {
    let bin = serde_json::to_string(value).expect("compact json for .bin");
    fs::write(dir.join(format!("{name}.bin")), bin).expect("write .bin (text)");
    let pretty = serde_json::to_string_pretty(value).expect("pretty json for .json");
    fs::write(dir.join(format!("{name}.json")), pretty).expect("write .json (text)");
}

fn hello_json() -> serde_json::Value {
    serde_json::json!({
        "kind": "hello",
        "seq": 0,
        "cols": 80,
        "rows": 24,
        "cursor": {
            "x": 0, "y": 0,
            "shape": "block",
            "blinking": false,
            "visible": true,
        },
        "escape_caps": [
            "sgr", "cup", "ed", "el", "decset", "decrst",
            "alt-screen-1049", "bracketed-paste",
            "mouse-vt200", "mouse-btn-event", "mouse-any-event", "mouse-sgr-1006",
            "focus-events", "app-cursor-keys",
        ],
        "input_caps": ["text-utf8", "key-vt-encoded"],
    })
}

fn mode_change(seq: u32, added: Vec<String>, removed: Vec<String>) -> serde_json::Value {
    let mf = ModeFrame::new(seq, added, removed);
    serde_json::to_value(mf).expect("ModeFrame to_value")
}
