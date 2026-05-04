/// Fixed-capacity byte ring buffer that retains the most recent PTY output
/// for use as terminal scrollback.
///
/// The buffer holds at most `cap` bytes. Once full, each new byte overwrites
/// the oldest byte, so the contents always represent the trailing window of
/// everything that has been written.
///
/// `head` is the index where the next byte will be written, and `full`
/// flips to `true` the first time `head` wraps back to `0`.
pub struct RingBuffer {
    buf: Vec<u8>,
    cap: usize,
    head: usize,
    full: bool,
}

impl RingBuffer {
    /// Creates an empty ring buffer that retains up to `cap` bytes of
    /// the most recent output.
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            buf: vec![0u8; cap],
            cap,
            head: 0,
            full: false,
        }
    }

    /// Appends `data` to the buffer, overwriting the oldest bytes once the
    /// buffer is full.
    pub fn push(&mut self, data: &[u8]) {
        for &b in data {
            self.buf[self.head] = b;
            self.head = (self.head + 1) % self.cap;
            if self.head == 0 {
                self.full = true;
            }
        }
    }

    /// Returns the buffered bytes in chronological order, from oldest to
    /// newest.
    ///
    /// Before the buffer has wrapped, this is just the prefix that has been
    /// written so far. After wrapping, the bytes from `head` to the end of
    /// the storage are followed by the bytes from the start up to `head`.
    pub fn snapshot(&self) -> Vec<u8> {
        if !self.full {
            self.buf[..self.head].to_vec()
        } else {
            let mut out = Vec::with_capacity(self.cap);
            out.extend_from_slice(&self.buf[self.head..]);
            out.extend_from_slice(&self.buf[..self.head]);
            out
        }
    }
}
