//! Recyclable Vec<u8> pool for on_paint BGRA buffers. Caps total in-flight
//! capacity to keep daemon RSS bounded under sustained 60 fps load.
//!
//! Threading: `acquire` / `release` may be called from any thread. The
//! underlying `crossbeam_queue::ArrayQueue` is lock-free.

use crossbeam_queue::ArrayQueue;

/// Bounded recycler for BGRA frame buffers.
pub struct FrameBufferPool {
    free: ArrayQueue<Vec<u8>>,
}

impl FrameBufferPool {
    /// Creates a pool that retains up to `capacity` recycled buffers.
    /// Excess `release`s drop their buffer back to the global allocator.
    pub fn new(capacity: usize) -> Self {
        Self {
            free: ArrayQueue::new(capacity),
        }
    }

    /// Returns a Vec<u8> of at least `min_len` bytes, recycled if available.
    /// Caller is responsible for `release`ing it back when done.
    pub fn acquire(&self, min_len: usize) -> Vec<u8> {
        if let Some(mut buf) = self.free.pop() {
            if buf.capacity() >= min_len {
                buf.clear();
                buf.resize(min_len, 0);
                return buf;
            }
            // NOTE: capacity too small; drop and alloc fresh below.
            drop(buf);
        }
        vec![0u8; min_len]
    }

    /// Returns `buf` to the pool. If the pool is full, `buf` is dropped.
    pub fn release(&self, buf: Vec<u8>) {
        let _ = self.free.push(buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_returns_buffer_of_min_len() {
        let pool = FrameBufferPool::new(4);
        let buf = pool.acquire(1024);
        assert_eq!(buf.len(), 1024);
    }

    #[test]
    fn release_then_acquire_recycles() {
        let pool = FrameBufferPool::new(4);
        let mut buf = pool.acquire(1024);
        let ptr = buf.as_ptr();
        buf.fill(0xaa);
        pool.release(buf);

        let buf2 = pool.acquire(1024);
        assert_eq!(buf2.as_ptr(), ptr, "expected recycled buffer (same alloc)");
        // NOTE: cleared after acquire.
        assert_eq!(buf2[0], 0, "expected cleared bytes");
    }

    #[test]
    fn over_capacity_release_drops() {
        let pool = FrameBufferPool::new(1);
        let a = pool.acquire(64);
        let b = pool.acquire(64);
        pool.release(a);
        pool.release(b); // second release exceeds capacity; should drop silently
        // No panic = pass; pool still has 1 buffer:
        let _ = pool.acquire(64);
    }

    #[test]
    fn small_capacity_buffer_is_reallocated() {
        let pool = FrameBufferPool::new(4);
        let small = pool.acquire(64);
        pool.release(small);
        // Asking for a larger buffer should NOT recycle the 64-byte one
        let large = pool.acquire(8192);
        assert_eq!(large.len(), 8192);
    }
}
