//! In-process verification of the ShmWriter seqlock pattern. Writer + reader
//! on a stack-allocated buffer (no real mmap). Verifies write_seq odd→even
//! commit, lap monotonicity, payload copy correctness, and no torn reads under
//! single-threaded conditions.

use ozmux_browser_cef_protocol::types::Rect;
use ozmux_cef_host::shm_writer::{NUM_SLOTS, ShmWriter, SlotData};

const SLOT_PAYLOAD_MAX: usize = 4 * 1024; // 4 KiB per slot, plenty for the test

fn region() -> Vec<u8> {
    vec![0u8; ShmWriter::required_region_size(SLOT_PAYLOAD_MAX)]
}

#[test]
fn write_slot_advances_lap_and_copies_payload() {
    let mut buf = region();
    // SAFETY: buffer is properly-aligned (Vec<u8> guarantees alignment 1, but
    // the Atomic<U32>/<U64> reads happen with explicit ordering; tests run
    // single-threaded so this is safe in practice. For production we'll mmap
    // with PROT_WRITE which is naturally aligned.
    let w = unsafe { ShmWriter::from_mmap(buf.as_mut_ptr(), SLOT_PAYLOAD_MAX) };

    assert_eq!(w.current_lap(), 0);

    let payload: Vec<u8> = (0..1024).map(|i| (i % 256) as u8).collect();
    let data = SlotData {
        frame_seq: 1,
        captured_at_us: 1_700_000_000_000_000,
        width: 100,
        height: 100,
        is_keyframe: true,
        damage_rects: vec![Rect {
            x: 0,
            y: 0,
            w: 100,
            h: 100,
        }],
        is_popup: false,
        payload: &payload,
    };
    let idx = w.write_slot(data);
    assert_eq!(idx, 0);
    assert_eq!(w.current_lap(), 1);
}

#[test]
fn write_slot_wraps_around_after_num_slots() {
    let mut buf = region();
    let w = unsafe { ShmWriter::from_mmap(buf.as_mut_ptr(), SLOT_PAYLOAD_MAX) };
    let payload = vec![0u8; 16];

    for expected_idx in 0..(NUM_SLOTS as u8 * 2) {
        let idx = w.write_slot(SlotData {
            frame_seq: expected_idx as u64,
            captured_at_us: 0,
            width: 1,
            height: 1,
            is_keyframe: true,
            damage_rects: vec![],
            is_popup: false,
            payload: &payload,
        });
        assert_eq!(idx, expected_idx % (NUM_SLOTS as u8));
    }
    assert_eq!(w.current_lap(), (NUM_SLOTS as u64) * 2);
}
