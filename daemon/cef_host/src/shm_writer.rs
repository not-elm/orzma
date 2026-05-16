//! `cef_host`-side shared memory ring writer.
//!
//! Each activity has its own ring with `NUM_SLOTS` slots. The writer (CEF UI
//! thread) writes BGRA payload using a seqlock pattern (odd `write_seq` = in
//! progress, even = committed) and bumps `lap_count` atomically.
//!
//! Layout invariants:
//! - `SlotHeader` is `#[repr(C, align(64))]` to keep cache lines clean and
//!   cross-process layout-stable.
//! - One mmap region per activity: `RingHeader` followed by `NUM_SLOTS`
//!   contiguous slot blocks. Each slot block is `size_of::<SlotHeader>() +
//!   slot_payload_max` bytes.
//! - Writer never skips a slot. `reader_lease` (mentioned in earlier design
//!   iterations) is not used — reader (daemon side) handles stale reads via
//!   the seqlock retry pattern.

use ozmux_browser_cef_protocol::types::Rect;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering, fence};

/// Number of slots in each activity's ring.
pub const NUM_SLOTS: usize = 4;

/// Maximum number of damage rectangles stored per slot.
pub const MAX_DAMAGE_RECTS: usize = 16;

/// Per-slot header: seqlock counter followed by frame metadata.
///
/// `repr(C, align(64))` ensures cache-line alignment and cross-process
/// layout stability.
#[repr(C, align(64))]
pub struct SlotHeader {
    /// Odd = write in progress; even = committed. Readers spin on this.
    pub write_seq: AtomicU32,
    #[allow(dead_code)]
    _pad0: [u8; 60],
    /// Monotonically increasing frame sequence number.
    pub frame_seq: u64,
    /// Capture timestamp in microseconds (wall clock).
    pub captured_at_us: u64,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Non-zero if this is a full keyframe.
    pub is_keyframe: u8,
    #[allow(dead_code)]
    _pad1: [u8; 3],
    /// Number of valid entries in `damage_rects`.
    pub damage_rects_count: u32,
    /// Damage rectangles for this frame (up to `MAX_DAMAGE_RECTS`).
    pub damage_rects: [Rect; MAX_DAMAGE_RECTS],
    /// Non-zero if this frame originates from a popup widget.
    pub is_popup: u8,
    #[allow(dead_code)]
    _pad2: [u8; 3],
    /// Number of valid payload bytes immediately following this header.
    pub payload_len: u32,
    // Payload bytes follow immediately in memory at offset size_of::<SlotHeader>().
    #[allow(dead_code)]
    _payload: [u8; 0],
}

/// Fixed-size ring header at the start of the mmap region.
///
/// `lap_count` is the absolute write counter; `lap_count % NUM_SLOTS` gives
/// the next slot index to write.
#[repr(C)]
pub struct RingHeader {
    /// Absolute write counter. Incremented after each slot commit.
    pub lap_count: AtomicU64,
    #[allow(dead_code)]
    _pad: [u8; 56],
}

/// Single-writer shared memory ring.
///
/// One instance exists per browser activity. All writes come from the CEF UI
/// thread; no skip logic is needed on the write path.
pub struct ShmWriter {
    /// Pointer to the start of the mmap region (`RingHeader` followed by
    /// `NUM_SLOTS` contiguous slot blocks).
    base: *mut u8,
    /// `size_of::<SlotHeader>() + slot_payload_max`.
    slot_stride: usize,
    /// Max payload bytes per slot.
    slot_payload_max: usize,
}

// SAFETY: ShmWriter only writes from a single thread (CEF UI thread). The
// `*mut u8` is a stable mmap address that lives for the activity's lifetime.
unsafe impl Send for ShmWriter {}
// SAFETY: same rationale — the CEF UI thread holds exclusive write access.
unsafe impl Sync for ShmWriter {}

/// Frame data to be written into the next ring slot.
pub struct SlotData<'a> {
    /// Monotonically increasing frame sequence number.
    pub frame_seq: u64,
    /// Capture timestamp in microseconds (wall clock).
    pub captured_at_us: u64,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// True if this is a full keyframe (no delta).
    pub is_keyframe: bool,
    /// Damage rectangles for this frame.
    pub damage_rects: Vec<Rect>,
    /// True if this frame comes from a popup widget.
    pub is_popup: bool,
    /// Raw pixel payload (BGRA). Truncated to `slot_payload_max` if larger.
    pub payload: &'a [u8],
}

impl ShmWriter {
    /// Construct from a raw mmap base pointer. Caller must guarantee the region
    /// is at least `size_of::<RingHeader>() + NUM_SLOTS * (size_of::<SlotHeader>() +
    /// slot_payload_max)` bytes large.
    ///
    /// # Safety
    /// - `base` must point to a writable, properly-aligned mmap region of the
    ///   expected size.
    /// - The caller must ensure no other writer touches this region.
    pub unsafe fn from_mmap(base: *mut u8, slot_payload_max: usize) -> Self {
        let slot_stride = std::mem::size_of::<SlotHeader>() + slot_payload_max;
        Self {
            base,
            slot_stride,
            slot_payload_max,
        }
    }

    fn ring_header(&self) -> &RingHeader {
        // SAFETY: base + 0 is a valid &RingHeader by construction (Self::from_mmap).
        unsafe { &*(self.base as *const RingHeader) }
    }

    fn slot(&self, idx: usize) -> *mut SlotHeader {
        // SAFETY: idx is bounded by NUM_SLOTS by the writer's `lap % NUM_SLOTS`.
        unsafe {
            self.base
                .add(std::mem::size_of::<RingHeader>() + idx * self.slot_stride)
                as *mut SlotHeader
        }
    }

    /// Write the next frame. Writer must be called from the CEF UI thread only.
    ///
    /// Returns the slot index used (`lap_count % NUM_SLOTS` before increment).
    pub fn write_slot(&self, data: SlotData) -> u8 {
        let lap = self.ring_header().lap_count.load(Ordering::Acquire);
        let idx = (lap as usize) % NUM_SLOTS;
        let slot = self.slot(idx);
        // SAFETY: slot pointer is valid; we are the sole writer.
        unsafe {
            let s = (*slot).write_seq.load(Ordering::Relaxed);
            // Begin write: advance to next odd value.
            (*slot)
                .write_seq
                .store(s.wrapping_add(1), Ordering::Release);

            (*slot).frame_seq = data.frame_seq;
            (*slot).captured_at_us = data.captured_at_us;
            (*slot).width = data.width;
            (*slot).height = data.height;
            (*slot).is_keyframe = u8::from(data.is_keyframe);
            (*slot).is_popup = u8::from(data.is_popup);

            let n_rects = data.damage_rects.len().min(MAX_DAMAGE_RECTS);
            (*slot).damage_rects_count = n_rects as u32;
            for (i, r) in data.damage_rects.iter().take(n_rects).enumerate() {
                (*slot).damage_rects[i] = *r;
            }

            let copy_len = data.payload.len().min(self.slot_payload_max);
            (*slot).payload_len = copy_len as u32;
            let payload_ptr = (slot as *mut u8).add(std::mem::size_of::<SlotHeader>());
            std::ptr::copy_nonoverlapping(data.payload.as_ptr(), payload_ptr, copy_len);

            fence(Ordering::Release);
            // Commit: advance to next even value.
            (*slot)
                .write_seq
                .store(s.wrapping_add(2), Ordering::Release);
        }
        self.ring_header().lap_count.fetch_add(1, Ordering::AcqRel);
        idx as u8
    }

    /// Returns the current absolute write counter (number of slots committed so far).
    pub fn current_lap(&self) -> u64 {
        self.ring_header().lap_count.load(Ordering::Acquire)
    }

    /// Returns the mmap region size (in bytes) required for the given payload capacity.
    pub fn required_region_size(slot_payload_max: usize) -> usize {
        std::mem::size_of::<RingHeader>()
            + NUM_SLOTS * (std::mem::size_of::<SlotHeader>() + slot_payload_max)
    }
}
