//! Test-support seam: an in-memory sink observing the PTY write path,
//! a scriptable [`Vt`] fake, plus crate-internal `MasterPty` fakes for
//! the resize seam.

use orzma_vt::prelude::{DisplayOffset, Frame, GridSize, Scroll, Vt, VtModes, InterpretOutput};
#[cfg(test)]
use portable_pty::{MasterPty, PtySize};
use std::collections::VecDeque;
#[cfg(test)]
use std::io::Read;
use std::io::{Result as IoResult, Write};
#[cfg(test)]
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Cloneable in-memory `Write` sink capturing every byte written to it.
///
/// Clones share one buffer: hand one clone to [`crate::OrzmaTty::detached`]
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

/// Scriptable [`Vt`] fake for exercising `OrzmaTty` without a real
/// emulator.
///
/// `interpret` records each chunk and pops the next scripted update; an
/// empty script yields an update with `damaged: true`, matching the
/// window-arming behavior of a real interpreted chunk when no test
/// script overrides it. `resize` applies honestly
/// (returns whether the size changed). `scroll` records the motion:
/// `Scroll::Bottom` snaps `display_offset` to zero, every other motion
/// returns the scripted `scroll_moves`.
pub struct FakeVt {
    /// Grid size reported and updated by `resize`.
    pub grid_size: GridSize,
    /// Offset reported by `display_offset`; `Scroll::Bottom` zeroes it.
    pub display_offset: DisplayOffset,
    /// Modes reported to the input encoders.
    pub modes: VtModes,
    /// Scripted return for non-`Bottom` scrolls.
    pub scroll_moves: bool,
    /// Every chunk `interpret` received, in order.
    pub interpreted: Vec<Vec<u8>>,
    /// Every motion `scroll` received, in order.
    pub scrolls: Vec<Scroll>,
    /// Every size `resize` received, in order.
    pub resizes: Vec<GridSize>,
    /// Updates popped by `interpret`.
    pub updates: VecDeque<InterpretOutput>,
    /// Frames popped by `frame`.
    pub frames: VecDeque<Frame>,
}

impl FakeVt {
    /// Builds a fake at the given grid size, at the live tail, with
    /// default modes and an empty script.
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            grid_size: GridSize { cols, rows },
            display_offset: DisplayOffset(0),
            modes: VtModes::default(),
            scroll_moves: false,
            interpreted: Vec::new(),
            scrolls: Vec::new(),
            resizes: Vec::new(),
            updates: VecDeque::new(),
            frames: VecDeque::new(),
        }
    }
}

impl Vt for FakeVt {
    fn interpret(&mut self, chunk: &[u8]) -> InterpretOutput {
        self.interpreted.push(chunk.to_vec());
        self.updates.pop_front().unwrap_or(InterpretOutput {
            damaged: true,
            signals: Vec::new(),
            replies: Vec::new(),
        })
    }

    fn frame(&mut self) -> Option<Frame> {
        self.frames.pop_front()
    }

    fn resize(&mut self, size: GridSize) -> bool {
        self.resizes.push(size);
        let changed = self.grid_size != size;
        self.grid_size = size;
        changed
    }

    fn scroll(&mut self, scroll: Scroll) -> bool {
        let snaps_to_bottom = matches!(scroll, Scroll::Bottom);
        self.scrolls.push(scroll);
        if snaps_to_bottom {
            let moved = self.display_offset != DisplayOffset(0);
            self.display_offset = DisplayOffset(0);
            moved
        } else {
            self.scroll_moves
        }
    }

    fn grid_size(&self) -> GridSize {
        self.grid_size
    }

    fn display_offset(&self) -> DisplayOffset {
        self.display_offset
    }

    fn modes(&self) -> VtModes {
        self.modes
    }
}

/// `MasterPty` whose `resize` always fails, for pinning failure paths
/// (`Pty::resize` error mapping, `OrzmaTty::resize` atomicity).
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
///
/// It also answers `get_size` with whatever it was last resized to, so
/// a caller that writes a size and reads it back sees what a real
/// master would.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct RecordingMaster {
    calls: Arc<Mutex<Vec<PtySize>>>,
    size: Mutex<PtySize>,
}

#[cfg(test)]
impl RecordingMaster {
    /// Builds the fake at `initial`, plus the shared handle its
    /// `resize` calls are recorded into.
    pub(crate) fn new(initial: PtySize) -> (Self, Arc<Mutex<Vec<PtySize>>>) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                calls: calls.clone(),
                size: Mutex::new(initial),
            },
            calls,
        )
    }

    /// Builds the fake at `cols` x `rows` with zero pixel dimensions.
    pub(crate) fn at(cols: u16, rows: u16) -> (Self, Arc<Mutex<Vec<PtySize>>>) {
        Self::new(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
    }
}

#[cfg(test)]
impl MasterPty for RecordingMaster {
    fn resize(&self, size: PtySize) -> anyhow::Result<()> {
        self.calls.lock().unwrap().push(size);
        *self.size.lock().unwrap() = size;
        Ok(())
    }

    fn get_size(&self) -> anyhow::Result<PtySize> {
        Ok(*self.size.lock().unwrap())
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
