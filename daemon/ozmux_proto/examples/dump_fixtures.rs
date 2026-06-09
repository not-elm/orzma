//! Dumps representative wire JSON so the Dart client can golden-test its DTOs
//! against the real serde shapes. Run: `cargo run -p ozmux_proto --example
//! dump_fixtures -- gui/test/fixtures/welcome.json gui/test/fixtures/events_split.json`

use ozmux_mux::{Multiplexer, Side, SplitOrientation, SurfaceKind};
use ozmux_proto::ServerMessage;

fn main() {
    let mut mux = Multiplexer::new();
    let session = mux.sessions()[0];
    let ws = mux.active_workspace();
    mux.set_workspace_size(ws, 120, 40).unwrap();
    let pane = mux.active_pane(ws).unwrap();
    let split_events = mux
        .split_pane(
            pane,
            SplitOrientation::Horizontal,
            Side::After,
            SurfaceKind::Terminal,
            None,
        )
        .unwrap();
    let snapshot = mux.snapshot(session).unwrap();

    let args: Vec<String> = std::env::args().collect();
    std::fs::write(
        &args[1],
        serde_json::to_vec_pretty(&ServerMessage::Welcome { snapshot }).unwrap(),
    )
    .unwrap();
    std::fs::write(
        &args[2],
        serde_json::to_vec_pretty(&ServerMessage::Events(split_events)).unwrap(),
    )
    .unwrap();
    eprintln!("wrote {} and {}", args[1], args[2]);
}
