//! Cross-thread integration test: cef_host::shm_writer + daemon::shm_reader
//! share a mmap region via Vec<u8> (in-process simulation; real cross-process
//! testing happens in the cef_host_handshake integration test in Task 22).

use bytes::Bytes;
use ozmux_browser::shm_reader::{NUM_SLOTS, ShmReader};
use ozmux_browser_cef_protocol::types::Rect;
use ozmux_cef_host::shm_writer::{ShmWriter, SlotData};

const SLOT_PAYLOAD_MAX: usize = 4 * 1024;

fn make_region() -> Vec<u8> {
    vec![0u8; ShmWriter::required_region_size(SLOT_PAYLOAD_MAX)]
}

#[test]
fn reader_sees_writer_keyframe() {
    let mut region = make_region();
    let w = unsafe { ShmWriter::from_mmap(region.as_mut_ptr(), SLOT_PAYLOAD_MAX) };
    let r = unsafe { ShmReader::from_mmap(region.as_ptr(), SLOT_PAYLOAD_MAX) };

    assert_eq!(r.current_lap(), 0);

    let payload: Vec<u8> = (0..1024).map(|i| (i % 256) as u8).collect();
    let idx = w.write_slot(SlotData {
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
    });
    assert_eq!(idx, 0);
    assert_eq!(r.current_lap(), 1);

    let snap = r.read_stable(0).expect("stable read");
    assert_eq!(snap.frame_seq, 1);
    assert_eq!(snap.captured_at_us, 1_700_000_000_000_000);
    assert_eq!(snap.width, 100);
    assert_eq!(snap.height, 100);
    assert!(snap.is_keyframe);
    assert!(!snap.is_popup);
    assert_eq!(snap.damage_rects.len(), 1);
    assert_eq!(
        snap.damage_rects[0],
        Rect {
            x: 0,
            y: 0,
            w: 100,
            h: 100
        }
    );
    assert_eq!(snap.payload, Bytes::copy_from_slice(&payload));
}

#[test]
fn reader_handles_wrap_around() {
    let mut region = make_region();
    let w = unsafe { ShmWriter::from_mmap(region.as_mut_ptr(), SLOT_PAYLOAD_MAX) };
    let r = unsafe { ShmReader::from_mmap(region.as_ptr(), SLOT_PAYLOAD_MAX) };
    let payload = vec![0xAAu8; 256];

    for i in 0..(NUM_SLOTS * 2) {
        let idx = w.write_slot(SlotData {
            frame_seq: i as u64,
            captured_at_us: i as u64,
            width: 1,
            height: 1,
            is_keyframe: true,
            damage_rects: vec![],
            is_popup: false,
            payload: &payload,
        });
        assert_eq!(idx, (i % NUM_SLOTS) as u8);
    }
    assert_eq!(r.current_lap(), (NUM_SLOTS * 2) as u64);

    for i in 0..NUM_SLOTS {
        let snap = r.read_stable(i).expect("stable read");
        assert_eq!(snap.frame_seq, (i + NUM_SLOTS) as u64);
    }
}

#[test]
fn reader_returns_none_when_writer_in_progress() {
    let mut region = make_region();
    let w = unsafe { ShmWriter::from_mmap(region.as_mut_ptr(), SLOT_PAYLOAD_MAX) };

    let payload = vec![0u8; 16];
    let _ = w.write_slot(SlotData {
        frame_seq: 99,
        captured_at_us: 0,
        width: 1,
        height: 1,
        is_keyframe: true,
        damage_rects: vec![],
        is_popup: false,
        payload: &payload,
    });

    // After a complete write, slot 0's write_seq is even (incremented twice: 0→1→2).
    // Manually patch it to odd to simulate a mid-write state.
    let r = unsafe { ShmReader::from_mmap(region.as_ptr(), SLOT_PAYLOAD_MAX) };
    let slot0_off = std::mem::size_of::<std::sync::atomic::AtomicU64>() + 56; // RingHeader size
    // SAFETY: slot0_off is within the region and points to the write_seq AtomicU32
    // of the first slot; we set it to an odd value to simulate in-progress write.
    unsafe {
        let write_seq_ptr = region.as_mut_ptr().add(slot0_off) as *mut std::sync::atomic::AtomicU32;
        (*write_seq_ptr).store(3, std::sync::atomic::Ordering::Release);
    }
    assert!(r.read_stable(0).is_none());
}
