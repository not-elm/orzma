//! Wire-contract round-trip test: every binary fixture decodes as `RenderFrame`
//! and re-encodes to byte-identical msgpack via `vt::encode`.
//!
//! Text fixtures (`hello.bin`) are excluded — they are JSON text, not msgpack.
//! The TypeScript verifier at `tools/verify-msgpack.ts` handles those.
use ozmux_terminal::vt::{RenderFrame, encode};
use std::fs;
use std::path::{Path, PathBuf};

const FIXTURE_DIR: &str = "tests/fixtures/wire_msgpack";

const TEXT_FIXTURES: &[&str] = &["hello"];

fn binary_fixtures() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE_DIR);
    let mut out: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read_dir {dir:?}: {e}"))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("bin"))
        .filter(|p| {
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            !TEXT_FIXTURES.contains(&stem)
        })
        .collect();
    out.sort();
    out
}

#[test]
fn every_binary_fixture_round_trips_as_render_frame() {
    let fixtures = binary_fixtures();
    assert!(
        !fixtures.is_empty(),
        "no binary fixtures discovered under {FIXTURE_DIR}"
    );
    let mut failed = Vec::new();
    for path in &fixtures {
        let original = fs::read(path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
        let frame: RenderFrame = match rmp_serde::from_slice(&original) {
            Ok(f) => f,
            Err(e) => {
                failed.push(format!("{path:?}: decode failed: {e}"));
                continue;
            }
        };
        let reencoded = encode(&frame).unwrap_or_else(|e| panic!("encode {path:?}: {e}"));
        if original != reencoded {
            failed.push(format!(
                "{path:?}: byte mismatch (original {} bytes, reencoded {} bytes)",
                original.len(),
                reencoded.len(),
            ));
        }
    }
    assert!(
        failed.is_empty(),
        "fixtures failed round-trip:\n{}",
        failed.join("\n")
    );
}
