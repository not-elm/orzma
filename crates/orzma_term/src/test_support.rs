//! Test-support seam: an in-memory sink observing the PTY write path.

use std::io::{Result as IoResult, Write};
use std::sync::{Arc, Mutex};

/// Cloneable in-memory `Write` sink capturing every byte written to it.
///
/// Clones share one buffer: hand one clone to [`crate::OrzmaTerm::detached`]
/// as the PTY writer and keep another to assert on [`CaptureSink::contents`].
#[derive(Clone, Default)]
pub struct CaptureSink(Arc<Mutex<Vec<u8>>>);

impl CaptureSink {
    /// Returns a copy of every byte written so far, in write order.
    pub fn contents(&self) -> Vec<u8> {
        self.0.lock().unwrap().clone()
    }
}

impl Write for CaptureSink {
    fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> IoResult<()> {
        Ok(())
    }
}
