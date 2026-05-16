//! Daemon-side shared memory reader. Mirrors `cef_host::shm_writer::SlotHeader`
//! and reads frames with the seqlock stable-read pattern.
//!
//! Layout MUST match `cef_host::shm_writer` byte-for-byte (`#[repr(C, align(64))]`,
//! identical field order and sizes).

use bytes::Bytes;
use ozmux_browser_cef_protocol::types::Rect;
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::atomic::Ordering;

/// Number of slots in each activity's main ring.
pub const NUM_SLOTS: usize = 4;

/// Number of popup slots (mirrors `cef_host::shm_writer::NUM_SLOTS_POPUP`).
pub const NUM_SLOTS_POPUP: usize = 1;

/// Maximum payload bytes for the popup slot (mirrors `cef_host::shm_writer::POPUP_PAYLOAD_MAX`).
pub const POPUP_PAYLOAD_MAX: usize = 800 * 600 * 4 + 4096;

/// Maximum number of damage rectangles stored per slot.
pub const MAX_DAMAGE_RECTS: usize = 16;

/// Must match `cef_host::shm_writer::SlotHeader` layout exactly.
#[repr(C, align(64))]
struct SlotHeader {
    write_seq: std::sync::atomic::AtomicU32,
    _pad0: [u8; 60],
    frame_seq: u64,
    captured_at_us: u64,
    width: u32,
    height: u32,
    is_keyframe: u8,
    _pad1: [u8; 3],
    damage_rects_count: u32,
    damage_rects: [Rect; MAX_DAMAGE_RECTS],
    is_popup: u8,
    _pad2: [u8; 3],
    payload_len: u32,
    // Payload bytes follow immediately in memory at offset size_of::<SlotHeader>().
}

/// Must match `cef_host::shm_writer::RingHeader` layout exactly.
#[repr(C)]
struct RingHeader {
    lap_count: std::sync::atomic::AtomicU64,
    _pad: [u8; 56],
}

/// Daemon-side reader for a shared memory ring written by the CEF host.
pub struct ShmReader {
    base: *const u8,
    slot_stride: usize,
}

// SAFETY: ShmReader reads only — atomics + volatile reads handle inter-thread
// visibility. Multiple readers are safe (no shared mutable state).
unsafe impl Send for ShmReader {}
unsafe impl Sync for ShmReader {}

/// Stable read of one frame slot, owned by the reader.
#[derive(Debug, Clone)]
pub struct FrameSnapshot {
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
    /// True if this frame originates from a popup widget.
    pub is_popup: bool,
    /// Damage rectangles for this frame.
    pub damage_rects: Vec<Rect>,
    /// Raw pixel payload (BGRA), owned copy.
    pub payload: Bytes,
}

impl ShmReader {
    /// Construct from a raw mmap base pointer.
    ///
    /// # Safety
    /// - `base` must point to a readable mmap region with the layout produced by
    ///   `cef_host::shm_writer::ShmWriter::from_mmap` using the same `slot_payload_max`.
    pub unsafe fn from_mmap(base: *const u8, slot_payload_max: usize) -> Self {
        let slot_stride = std::mem::size_of::<SlotHeader>() + slot_payload_max;
        Self { base, slot_stride }
    }

    fn ring_header(&self) -> &RingHeader {
        // SAFETY: base + 0 is a valid &RingHeader by the from_mmap contract.
        unsafe { &*(self.base as *const RingHeader) }
    }

    fn slot(&self, idx: usize) -> *const SlotHeader {
        // SAFETY: idx is bounded by NUM_SLOTS; the region is large enough by contract.
        unsafe {
            self.base
                .add(std::mem::size_of::<RingHeader>() + idx * self.slot_stride)
                as *const SlotHeader
        }
    }

    fn popup_slot(&self) -> *const SlotHeader {
        // SAFETY: the popup slot lives immediately after the main ring slots.
        unsafe {
            self.base
                .add(std::mem::size_of::<RingHeader>() + NUM_SLOTS * self.slot_stride)
                as *const SlotHeader
        }
    }

    /// Returns the current absolute write counter (number of slots committed so far).
    pub fn current_lap(&self) -> u64 {
        self.ring_header().lap_count.load(Ordering::Acquire)
    }

    /// Stable read of `slot_idx`. Retries up to 3 times if a writer interrupts.
    ///
    /// Returns `None` if the slot is in mid-write or the read is unstable after
    /// 3 retries.
    pub fn read_stable(&self, slot_idx: usize) -> Option<FrameSnapshot> {
        self.read_slot(self.slot(slot_idx))
    }

    /// Stable read of the fixed popup slot. Retries up to 3 times if a writer
    /// interrupts. Returns `None` when the slot is uninitialised or mid-write.
    pub fn read_popup(&self) -> Option<FrameSnapshot> {
        self.read_slot(self.popup_slot())
    }

    fn read_slot(&self, slot: *const SlotHeader) -> Option<FrameSnapshot> {
        for _retry in 0..3 {
            // SAFETY: slot pointer is valid by the from_mmap contract.
            let s1 = unsafe { (*slot).write_seq.load(Ordering::Acquire) };
            if s1 & 1 == 1 {
                return None;
            }

            let (frame_seq, captured_at_us, width, height, is_keyframe, is_popup, damage, payload);
            // SAFETY: slot pointer is valid; volatile reads defeat compiler caching.
            unsafe {
                frame_seq = std::ptr::read_volatile(&(*slot).frame_seq);
                captured_at_us = std::ptr::read_volatile(&(*slot).captured_at_us);
                width = std::ptr::read_volatile(&(*slot).width);
                height = std::ptr::read_volatile(&(*slot).height);
                is_keyframe = std::ptr::read_volatile(&(*slot).is_keyframe) != 0;
                is_popup = std::ptr::read_volatile(&(*slot).is_popup) != 0;
                let n = (std::ptr::read_volatile(&(*slot).damage_rects_count) as usize)
                    .min(MAX_DAMAGE_RECTS);
                damage = (0..n).map(|i| (*slot).damage_rects[i]).collect();
                let plen = std::ptr::read_volatile(&(*slot).payload_len) as usize;
                let payload_ptr = (slot as *const u8).add(std::mem::size_of::<SlotHeader>());
                payload = Bytes::copy_from_slice(std::slice::from_raw_parts(payload_ptr, plen));
            }
            std::sync::atomic::fence(Ordering::Acquire);
            // SAFETY: slot pointer is valid.
            let s2 = unsafe { (*slot).write_seq.load(Ordering::Acquire) };
            if s1 == s2 {
                return Some(FrameSnapshot {
                    frame_seq,
                    captured_at_us,
                    width,
                    height,
                    is_keyframe,
                    is_popup,
                    damage_rects: damage,
                    payload,
                });
            }
        }
        None
    }

    /// Returns the mmap region size (in bytes) required for the given payload capacity.
    ///
    /// Includes the main ring (`NUM_SLOTS` slots) and the popup slot
    /// (`NUM_SLOTS_POPUP` slots at `POPUP_PAYLOAD_MAX` each). Must stay in
    /// sync with `cef_host::shm_writer::ShmWriter::required_region_size`.
    pub fn required_region_size(slot_payload_max: usize) -> usize {
        std::mem::size_of::<RingHeader>()
            + NUM_SLOTS * (std::mem::size_of::<SlotHeader>() + slot_payload_max)
            + NUM_SLOTS_POPUP * (std::mem::size_of::<SlotHeader>() + POPUP_PAYLOAD_MAX)
    }
}

/// A [`ShmReader`] that owns its `mmap` region and unmaps it on drop.
///
/// `ShmReader::from_mmap` borrows a raw base pointer it does not own;
/// `OwnedShmReader` pairs the reader with the mapping so the region stays
/// valid for the reader's lifetime and is released when the activity closes.
pub struct OwnedShmReader {
    base: *mut std::ffi::c_void,
    len: usize,
    reader: ShmReader,
}

// SAFETY: the mapping is `PROT_READ` shared memory; `ShmReader` already
// guarantees thread-safe reads, and `munmap` on drop touches the region only
// once no reader remains.
unsafe impl Send for OwnedShmReader {}
unsafe impl Sync for OwnedShmReader {}

impl OwnedShmReader {
    /// Maps `fd` read-only and wraps a [`ShmReader`] over the mapping.
    ///
    /// `fd` is only borrowed for the `mmap` call — the mapping outlives the
    /// descriptor, so the caller may move `fd` elsewhere (e.g. send it to
    /// cef_host) afterwards.
    pub fn map(fd: &OwnedFd, slot_payload_max: usize) -> std::io::Result<Self> {
        let len = ShmReader::required_region_size(slot_payload_max);
        // SAFETY: `fd` is a valid shm descriptor sized to at least `len` bytes
        // by `shm_alloc::create_shm_for_activity`.
        let base = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ,
                libc::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            )
        };
        if base == libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `base` is a valid mmap region of `len` bytes laid out per the
        // shared shm_writer / shm_reader contract for `slot_payload_max`.
        let reader = unsafe { ShmReader::from_mmap(base as *const u8, slot_payload_max) };
        Ok(Self { base, len, reader })
    }

    /// Returns the wrapped reader.
    pub fn reader(&self) -> &ShmReader {
        &self.reader
    }
}

impl Drop for OwnedShmReader {
    fn drop(&mut self) {
        // SAFETY: `base` / `len` came from this struct's own `mmap` call.
        unsafe {
            libc::munmap(self.base, self.len);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_region_size_grows_by_popup_slot() {
        let slot_payload_max = 1280 * 800 * 4 + 4096;
        let popup_extra = std::mem::size_of::<SlotHeader>() + POPUP_PAYLOAD_MAX;
        let with_popup = ShmReader::required_region_size(slot_payload_max);
        let without_popup = std::mem::size_of::<RingHeader>()
            + NUM_SLOTS * (std::mem::size_of::<SlotHeader>() + slot_payload_max);
        assert_eq!(with_popup, without_popup + popup_extra);
    }
}
