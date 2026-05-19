//! Generates the synthetic_scroll_burst.tape fixture deterministically
//! (no RNG, no time, no IO other than the output path).
//!
//! Usage:
//!   cargo run --features test-helpers -p ozmux_terminal --example generate_tape \
//!     -- --out daemon/terminal/tests/fixtures/pty_tapes/synthetic_scroll_burst.tape
use ozmux_terminal::testing::tape::{TapeRecord, write_tape};
use std::path::PathBuf;

fn build_records() -> Vec<TapeRecord> {
    let mut records: Vec<TapeRecord> = Vec::new();
    let mut ts: u64 = 0;
    let bump = |ts: &mut u64, ns: u64| {
        *ts += ns;
        *ts
    };

    // Phase 0: alt-screen enter, hide cursor, clear
    records.push(TapeRecord {
        ts_ns_offset: bump(&mut ts, 0),
        bytes: b"\x1b[?1049h\x1b[?25l\x1b[H\x1b[2J".to_vec(),
    });

    // Phase 1: 100 frames of scroll burst — full-screen text writes
    for frame in 0..100 {
        let mut chunk = Vec::with_capacity(4096);
        for row in 0..24 {
            chunk.extend_from_slice(format!("\x1b[{};1H", row + 1).as_bytes());
            chunk.extend_from_slice(format!("line {:04}", frame * 24 + row).as_bytes());
            chunk.extend_from_slice(b"\x1b[K"); // clear to end of line
        }
        records.push(TapeRecord {
            ts_ns_offset: bump(&mut ts, 16_000_000), // ~60fps
            bytes: chunk,
        });
    }

    // Phase 2: 10 frames with OSC 8 hyperlinks
    for i in 0..10 {
        let chunk = format!(
            "\x1b[1;1H\x1b]8;;https://example.com/{i}\x1b\\link-{i}\x1b]8;;\x1b\\"
        )
        .into_bytes();
        records.push(TapeRecord {
            ts_ns_offset: bump(&mut ts, 16_000_000),
            bytes: chunk,
        });
    }

    // Phase 3: 5 mode-change cycles
    for _ in 0..5 {
        records.push(TapeRecord {
            ts_ns_offset: bump(&mut ts, 4_000_000),
            bytes: b"\x1b[?1006h".to_vec(),
        });
        records.push(TapeRecord {
            ts_ns_offset: bump(&mut ts, 4_000_000),
            bytes: b"\x1b[?1006l".to_vec(),
        });
    }

    // Phase 4: 5 records of 1-16 bytes (stress test the chunking path)
    for i in 0..5u8 {
        records.push(TapeRecord {
            ts_ns_offset: bump(&mut ts, 1_000_000),
            bytes: vec![0x41 + i],
        });
    }

    // Phase 5: cleanup
    records.push(TapeRecord {
        ts_ns_offset: bump(&mut ts, 0),
        bytes: b"\x1b[?1049l\x1b[?25h".to_vec(),
    });

    records
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out_path = args
        .iter()
        .position(|a| a == "--out")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
        .expect("usage: generate_tape --out <path>");

    let records = build_records();
    write_tape(&out_path, &records).expect("write_tape failed");
    eprintln!(
        "generate_tape: wrote {} records to {:?}",
        records.len(),
        out_path
    );
}
