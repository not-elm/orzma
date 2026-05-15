use std::num::NonZeroUsize;

/// Fixed-capacity byte ring buffer that retains the most recent PTY output
/// for use as terminal scrollback.
///
/// The buffer holds at most `capacity` bytes. Once full, each new byte
/// overwrites the oldest byte, so the contents always represent the trailing
/// window of everything that has been written.
#[derive(Debug)]
pub(super) struct RingBuffer {
    buf: Box<[u8]>,
    head: usize,
    full: bool,
}

impl RingBuffer {
    /// Creates an empty ring buffer that retains up to `capacity` bytes of
    /// the most recent output.
    pub fn with_capacity(capacity: NonZeroUsize) -> Self {
        Self {
            buf: vec![0u8; capacity.get()].into_boxed_slice(),
            head: 0,
            full: false,
        }
    }

    /// Appends `data` to the buffer, overwriting the oldest bytes when full.
    ///
    /// If `data` is longer than the capacity, only its trailing `capacity`
    /// bytes are retained.
    pub fn push(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        let cap = self.buf.len();

        // Anything before the trailing `cap` bytes would be immediately
        // overwritten, so collapse to a single contiguous copy.
        if data.len() >= cap {
            self.buf.copy_from_slice(&data[data.len() - cap..]);
            self.head = 0;
            self.full = true;
            return;
        }

        let tail_room = cap - self.head;
        if data.len() <= tail_room {
            self.buf[self.head..self.head + data.len()].copy_from_slice(data);
            self.head += data.len();
            if self.head == cap {
                self.head = 0;
                self.full = true;
            }
        } else {
            let (front, back) = data.split_at(tail_room);
            self.buf[self.head..].copy_from_slice(front);
            self.buf[..back.len()].copy_from_slice(back);
            self.head = back.len();
            self.full = true;
        }
    }

    /// Returns the buffered bytes as up to two contiguous slices in
    /// chronological order: the older slice first, then the newer one.
    ///
    /// Use this in hot paths to avoid the allocation that [`Self::snapshot`]
    /// performs.
    pub fn as_slices(&self) -> (&[u8], &[u8]) {
        if !self.full {
            (&self.buf[..self.head], &[])
        } else {
            let (older_tail, newer_head) = self.buf.split_at(self.head);
            (newer_head, older_tail)
        }
    }

    /// Returns the buffered bytes in chronological order, oldest to newest.
    ///
    /// Allocates a fresh `Vec`. Prefer [`Self::as_slices`] when the caller can
    /// consume the bytes without owning them.
    pub fn snapshot(&self) -> Vec<u8> {
        let (a, b) = self.as_slices();
        let mut out = Vec::with_capacity(a.len() + b.len());
        out.extend_from_slice(a);
        out.extend_from_slice(b);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cap(n: usize) -> NonZeroUsize {
        NonZeroUsize::new(n).unwrap()
    }

    #[test]
    fn empty_buffer_yields_nothing() {
        let rb = RingBuffer::with_capacity(cap(8));
        assert_eq!(rb.snapshot(), b"");
        assert_eq!(rb.as_slices(), (&b""[..], &b""[..]));
    }

    #[test]
    fn empty_push_is_noop() {
        let mut rb = RingBuffer::with_capacity(cap(4));
        rb.push(b"");
        assert_eq!(rb.snapshot(), b"");
        rb.push(b"ab");
        rb.push(b"");
        assert_eq!(rb.snapshot(), b"ab");
    }

    #[test]
    fn partial_fill_returns_prefix() {
        let mut rb = RingBuffer::with_capacity(cap(8));
        rb.push(b"abc");
        assert_eq!(rb.snapshot(), b"abc");
        let (a, b) = rb.as_slices();
        assert_eq!(a, b"abc");
        assert_eq!(b, b"");
    }

    #[test]
    fn exact_fill_to_boundary() {
        let mut rb = RingBuffer::with_capacity(cap(4));
        rb.push(b"abcd");
        assert_eq!(rb.snapshot(), b"abcd");
    }

    #[test]
    fn single_byte_wrap() {
        let mut rb = RingBuffer::with_capacity(cap(4));
        rb.push(b"abcd");
        rb.push(b"e");
        assert_eq!(rb.snapshot(), b"bcde");
    }

    #[test]
    fn wrap_inside_one_push() {
        let mut rb = RingBuffer::with_capacity(cap(4));
        rb.push(b"ab");
        rb.push(b"cdef");
        assert_eq!(rb.snapshot(), b"cdef");
    }

    #[test]
    fn wrap_across_pushes_with_residual_old_bytes() {
        let mut rb = RingBuffer::with_capacity(cap(4));
        rb.push(b"XY");
        rb.push(b"abc");
        assert_eq!(rb.snapshot(), b"Yabc");
    }

    #[test]
    fn input_longer_than_capacity_keeps_tail() {
        let mut rb = RingBuffer::with_capacity(cap(4));
        rb.push(b"abcdefghij");
        assert_eq!(rb.snapshot(), b"ghij");
    }

    #[test]
    fn input_far_longer_than_capacity_after_partial_state() {
        let mut rb = RingBuffer::with_capacity(cap(4));
        rb.push(b"x");
        rb.push(b"abcdefghij");
        assert_eq!(rb.snapshot(), b"ghij");
    }

    #[test]
    fn capacity_one_overwrites_each_byte() {
        let mut rb = RingBuffer::with_capacity(cap(1));
        rb.push(b"a");
        assert_eq!(rb.snapshot(), b"a");
        rb.push(b"b");
        assert_eq!(rb.snapshot(), b"b");
        rb.push(b"cde");
        assert_eq!(rb.snapshot(), b"e");
    }

    #[test]
    fn snapshot_matches_concatenated_slices() {
        let mut rb = RingBuffer::with_capacity(cap(6));
        rb.push(b"hello, world");
        let (a, b) = rb.as_slices();
        let mut joined = Vec::new();
        joined.extend_from_slice(a);
        joined.extend_from_slice(b);
        assert_eq!(joined, rb.snapshot());
        assert_eq!(rb.snapshot(), b" world");
    }
}
