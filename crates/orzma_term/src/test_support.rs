//! Test-support seam: an in-memory sink observing the PTY write path,
//! plus crate-internal `MasterPty` fakes for the resize seam.

#[cfg(test)]
use portable_pty::{MasterPty, PtySize};
#[cfg(test)]
use std::io::Read;
use std::io::{Result as IoResult, Write};
#[cfg(test)]
use std::path::PathBuf;
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

/// `MasterPty` whose `resize` always fails, for pinning failure paths
/// (`Pty::resize` error mapping, `OrzmaTerm::resize` atomicity).
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct FailingMaster;

#[cfg(test)]
impl MasterPty for FailingMaster {
    fn resize(&self, _size: PtySize) -> anyhow::Result<()> {
        Err(anyhow::anyhow!("injected resize failure"))
    }

    fn get_size(&self) -> anyhow::Result<PtySize> {
        Err(anyhow::anyhow!("not implemented for FailingMaster"))
    }

    fn try_clone_reader(&self) -> anyhow::Result<Box<dyn Read + Send>> {
        Err(anyhow::anyhow!("not implemented for FailingMaster"))
    }

    fn take_writer(&self) -> anyhow::Result<Box<dyn Write + Send>> {
        Err(anyhow::anyhow!("not implemented for FailingMaster"))
    }

    #[cfg(unix)]
    fn process_group_leader(&self) -> Option<i32> {
        None
    }

    #[cfg(unix)]
    fn as_raw_fd(&self) -> Option<std::os::unix::io::RawFd> {
        None
    }

    #[cfg(unix)]
    fn tty_name(&self) -> Option<PathBuf> {
        None
    }
}

/// `MasterPty` recording every `PtySize` handed to `resize`, so tests
/// can assert the exact struct the caller forwarded (field mapping and
/// pixel-zero policy are unobservable through kernel readback alone).
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct RecordingMaster(Arc<Mutex<Vec<PtySize>>>);

#[cfg(test)]
impl RecordingMaster {
    /// Builds the fake plus the shared handle its `resize` calls are
    /// recorded into.
    pub(crate) fn new() -> (Self, Arc<Mutex<Vec<PtySize>>>) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        (Self(calls.clone()), calls)
    }
}

#[cfg(test)]
impl MasterPty for RecordingMaster {
    fn resize(&self, size: PtySize) -> anyhow::Result<()> {
        self.0.lock().unwrap().push(size);
        Ok(())
    }

    fn get_size(&self) -> anyhow::Result<PtySize> {
        Err(anyhow::anyhow!("not implemented for RecordingMaster"))
    }

    fn try_clone_reader(&self) -> anyhow::Result<Box<dyn Read + Send>> {
        Err(anyhow::anyhow!("not implemented for RecordingMaster"))
    }

    fn take_writer(&self) -> anyhow::Result<Box<dyn Write + Send>> {
        Err(anyhow::anyhow!("not implemented for RecordingMaster"))
    }

    #[cfg(unix)]
    fn process_group_leader(&self) -> Option<i32> {
        None
    }

    #[cfg(unix)]
    fn as_raw_fd(&self) -> Option<std::os::unix::io::RawFd> {
        None
    }

    #[cfg(unix)]
    fn tty_name(&self) -> Option<PathBuf> {
        None
    }
}
