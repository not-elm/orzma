//! Test-support seam: fakes that drive [`crate::OrzmaTty`] without a real
//! PTY, child process, or VT emulator.

#[cfg(any(test, feature = "test-support"))]
use crate::CellPixels;
use orzma_vt::prelude::{
    CellSide, DisplayOffset, Frame, GridColumn, GridPoint, GridSize, InstanceId, InterpretOutput,
    KittyKeyboardFlags, PlacementSize, ResizeChanged, ScreenLine, Scroll, SelectionKind,
    SynchronizedOutput, ViCursor, ViModeSwitch, ViMotion, Vt, VtModes,
};
#[cfg(any(test, feature = "test-support"))]
use portable_pty::{MasterPty, PtySize};
use std::collections::VecDeque;
#[cfg(any(test, feature = "test-support"))]
use std::io::Read;
use std::io::{Error as IoError, ErrorKind, Result as IoResult, Write};
#[cfg(all(unix, any(test, feature = "test-support")))]
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

/// Cloneable in-memory `Write` sink capturing every byte written to it
/// and counting the `write` calls that delivered them.
///
/// Clones share one buffer, so a clone handed out as the PTY writer and a
/// clone kept for [`CaptureSink::contents`] see the same bytes.
#[derive(Clone, Default)]
pub struct CaptureSink(Arc<Mutex<(Vec<u8>, usize)>>);

impl CaptureSink {
    /// Returns a copy of every byte written so far, in write order.
    pub fn contents(&self) -> Vec<u8> {
        self.0.lock().unwrap().0.clone()
    }

    /// Returns how many `write` calls have landed so far.
    pub fn writes(&self) -> usize {
        self.0.lock().unwrap().1
    }
}

impl Write for CaptureSink {
    fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
        let mut captured = self.0.lock().unwrap();
        captured.0.extend_from_slice(buf);
        captured.1 += 1;
        Ok(buf.len())
    }

    fn flush(&mut self) -> IoResult<()> {
        Ok(())
    }
}

/// A `Write` sink whose every write fails with `ErrorKind::BrokenPipe`.
pub struct FailingSink;

impl Write for FailingSink {
    fn write(&mut self, _buf: &[u8]) -> IoResult<usize> {
        Err(IoError::from(ErrorKind::BrokenPipe))
    }

    fn flush(&mut self) -> IoResult<()> {
        Ok(())
    }
}

/// A `Write` sink whose writes block until the sink is released, standing
/// in for a PTY whose application stopped reading stdin.
///
/// Clones share one gate, so releasing any clone unblocks the writes
/// pending in every clone. A released sink discards what it is given.
#[derive(Clone, Default)]
pub struct BlockingSink {
    /// Set once a write has entered any clone.
    entered: Arc<OnceLock<()>>,
    /// Set once any clone is released.
    released: Arc<OnceLock<()>>,
}

impl BlockingSink {
    /// Unblocks every pending and future write.
    pub fn release(&self) {
        let _ = self.released.set(());
    }

    /// Blocks until a write has entered this sink or one of its clones.
    pub fn wait_until_writing(&self) {
        self.entered.wait();
    }
}

impl Write for BlockingSink {
    fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
        let _ = self.entered.set(());
        self.released.wait();
        Ok(buf.len())
    }

    fn flush(&mut self) -> IoResult<()> {
        Ok(())
    }
}

/// A selection operation a [`FakeVt`] received.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionOp {
    /// `start_selection`, with its arguments.
    Start(GridPoint, CellSide, SelectionKind),
    /// `extend_selection`, with its arguments.
    Extend(GridPoint, CellSide),
    /// `clear_selection`.
    Clear,
}

/// Scriptable [`Vt`] fake for exercising `OrzmaTty` without a real
/// emulator.
///
/// `interpret` records each chunk and pops the next scripted update; an
/// empty script yields an update with `damaged: true` that consumed the
/// whole chunk. `resize` applies
/// honestly (`None` when the size did not change) and names the next
/// scripted `evictions` entry when it did.
///
/// `scroll` records the motion: `Scroll::Bottom` snaps the display offset
/// to zero, and every other motion returns the verdict set by
/// [`FakeVt::set_scroll_moves`]; a `Scroll::Delta` that verdict lets
/// through also moves the display offset by its delta.
/// The selection operations record themselves, readable through
/// [`FakeVt::selections`], and return the verdict set by
/// [`FakeVt::set_selection_changes`]; `selection_text` returns the text
/// set by [`FakeVt::set_selected_text`].
///
/// The script is fed through the `push_*` methods and the recordings are
/// read back through [`FakeVt::interpreted`], [`FakeVt::scrolls`],
/// [`FakeVt::resizes`] and [`FakeVt::mounts`].
pub struct FakeVt {
    /// Grid size reported and updated by `resize`.
    grid_size: GridSize,
    /// Offset reported by `display_offset`; `Scroll::Bottom` zeroes it, and
    /// a moving `Scroll::Delta` shifts it.
    display_offset: DisplayOffset,
    /// Device-wide terminal modes the host reads back.
    ///
    /// A mode that would show up in a frame — DECTCEM's cursor
    /// visibility, for one — does not reach the scripted frames; script
    /// the frame to match instead.
    modes: VtModes,
    /// Flags `kitty_keyboard_flags` reports.
    kitty_keyboard_flags: KittyKeyboardFlags,
    /// Scripted return for non-`Bottom` scrolls.
    scroll_moves: bool,
    /// Scripted return for the selection operations.
    selection_changes: bool,
    /// Every selection operation received, in order.
    selections: Vec<SelectionOp>,
    /// Returned by `selection_text`.
    selected_text: Option<String>,
    /// Every chunk `interpret` received, in order.
    interpreted: Vec<Vec<u8>>,
    /// Every motion `scroll` received, in order.
    scrolls: Vec<Scroll>,
    /// Every size `resize` received, in order.
    resizes: Vec<GridSize>,
    /// Updates popped by `interpret`.
    updates: VecDeque<InterpretOutput>,
    /// Frames popped by `frame`.
    frames: VecDeque<Frame>,
    /// Placements a `resize` that changed the size reports as
    /// stranded, popped one list per such resize; an empty script
    /// reports none.
    evictions: VecDeque<Vec<InstanceId>>,
    /// The synchronized-output state `modes` reports after each
    /// `interpret`, popped one per call; an empty script leaves the
    /// state as it is.
    sync_script: VecDeque<SynchronizedOutput>,
    /// Every host-driven mount received, in order.
    mounts: Vec<(ScreenLine, GridColumn, PlacementSize, InstanceId)>,
    /// Scripted verdict for host-driven mounts.
    mount_accepts: bool,
}

impl FakeVt {
    /// Builds a fake at the given grid size, at the live tail, with
    /// default modes and an empty script.
    pub fn new(grid_size: GridSize) -> Self {
        Self {
            grid_size,
            display_offset: DisplayOffset(0),
            modes: VtModes::default(),
            kitty_keyboard_flags: KittyKeyboardFlags::empty(),
            scroll_moves: false,
            selection_changes: false,
            selections: Vec::new(),
            selected_text: None,
            interpreted: Vec::new(),
            scrolls: Vec::new(),
            resizes: Vec::new(),
            updates: VecDeque::new(),
            frames: VecDeque::new(),
            evictions: VecDeque::new(),
            mounts: Vec::new(),
            mount_accepts: true,
            sync_script: VecDeque::new(),
        }
    }

    /// Replaces the grid size the fake reports.
    pub fn set_grid_size(&mut self, size: GridSize) {
        self.grid_size = size;
    }

    /// Replaces the display offset the fake reports.
    pub fn set_display_offset(&mut self, offset: DisplayOffset) {
        self.display_offset = offset;
    }

    /// Returns the terminal modes the host reads back, for mutation.
    pub fn modes_mut(&mut self) -> &mut VtModes {
        &mut self.modes
    }

    /// Sets the flags `kitty_keyboard_flags` reports.
    pub fn set_kitty_keyboard_flags(&mut self, flags: KittyKeyboardFlags) {
        self.kitty_keyboard_flags = flags;
    }

    /// Sets the verdict returned for non-`Bottom` scrolls.
    pub fn set_scroll_moves(&mut self, moves: bool) {
        self.scroll_moves = moves;
    }

    /// Sets the verdict returned for the selection operations.
    pub fn set_selection_changes(&mut self, changes: bool) {
        self.selection_changes = changes;
    }

    /// Sets the text `selection_text` returns.
    pub fn set_selected_text(&mut self, text: Option<String>) {
        self.selected_text = text;
    }

    /// Sets the verdict returned for host-driven mounts.
    pub fn set_mount_accepts(&mut self, accepts: bool) {
        self.mount_accepts = accepts;
    }

    /// Queues an update for `interpret` to pop.
    pub fn push_update(&mut self, update: InterpretOutput) {
        self.updates.push_back(update);
    }

    /// Queues a frame for `frame` to pop.
    pub fn push_frame(&mut self, frame: Frame) {
        self.frames.push_back(frame);
    }

    /// Queues the evicted placements the next size-changing `resize` reports.
    pub fn push_evictions(&mut self, evicted: Vec<InstanceId>) {
        self.evictions.push_back(evicted);
    }

    /// Queues the synchronized-output state the next `interpret` applies.
    pub fn push_sync_state(&mut self, state: SynchronizedOutput) {
        self.sync_script.push_back(state);
    }

    /// Returns every chunk `interpret` received, in order.
    pub fn interpreted(&self) -> &[Vec<u8>] {
        &self.interpreted
    }

    /// Returns every motion `scroll` received, in order.
    pub fn scrolls(&self) -> &[Scroll] {
        &self.scrolls
    }

    /// Returns every size `resize` received, in order.
    pub fn resizes(&self) -> &[GridSize] {
        &self.resizes
    }

    /// Returns every selection operation received, in order.
    pub fn selections(&self) -> &[SelectionOp] {
        &self.selections
    }

    /// Returns every host-driven mount received, in order.
    pub fn mounts(&self) -> &[(ScreenLine, GridColumn, PlacementSize, InstanceId)] {
        &self.mounts
    }

    /// Returns how many scripted frames `frame` has yet to pop.
    pub fn pending_frames(&self) -> usize {
        self.frames.len()
    }
}

impl Vt for FakeVt {
    fn interpret(&mut self, chunk: &[u8]) -> InterpretOutput {
        self.interpreted.push(chunk.to_vec());
        if let Some(state) = self.sync_script.pop_front() {
            self.modes.synchronized_output = state;
        }
        self.updates.pop_front().unwrap_or(InterpretOutput {
            damaged: true,
            signals: Vec::new(),
            replies: Vec::new(),
            consumed: chunk.len(),
            synchronized_update_closed: false,
        })
    }

    fn frame(&mut self) -> Option<Frame> {
        self.frames.pop_front()
    }

    fn mount_placement_at(
        &mut self,
        row: ScreenLine,
        column: GridColumn,
        size: PlacementSize,
        instance: InstanceId,
    ) -> bool {
        self.mounts.push((row, column, size, instance));
        self.mount_accepts
    }

    fn remove_placements(&mut self, _instances: &[InstanceId]) -> bool {
        false
    }

    fn resize(&mut self, size: GridSize) -> Option<ResizeChanged> {
        self.resizes.push(size);
        if self.grid_size == size {
            return None;
        }
        self.grid_size = size;
        Some(ResizeChanged {
            evicted: self.evictions.pop_front().unwrap_or_default(),
        })
    }

    fn scroll(&mut self, scroll: Scroll) -> bool {
        self.scrolls.push(scroll);
        match scroll {
            Scroll::Bottom => {
                let moved = self.display_offset != DisplayOffset(0);
                self.display_offset = DisplayOffset(0);
                moved
            }
            Scroll::Delta(delta) if self.scroll_moves => {
                self.display_offset =
                    DisplayOffset(self.display_offset.0.saturating_add_signed(delta));
                true
            }
            _ => self.scroll_moves,
        }
    }

    fn start_selection(&mut self, cell: GridPoint, side: CellSide, kind: SelectionKind) -> bool {
        self.selections.push(SelectionOp::Start(cell, side, kind));
        self.selection_changes
    }

    fn extend_selection(&mut self, cell: GridPoint, side: CellSide) -> bool {
        self.selections.push(SelectionOp::Extend(cell, side));
        self.selection_changes
    }

    fn clear_selection(&mut self) -> bool {
        self.selections.push(SelectionOp::Clear);
        self.selection_changes
    }

    fn selection_text(&self) -> Option<String> {
        self.selected_text.clone()
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

    fn kitty_keyboard_flags(&self) -> KittyKeyboardFlags {
        self.kitty_keyboard_flags
    }

    fn switch_vi_mode(&mut self, _switch: ViModeSwitch) -> bool {
        false
    }

    fn vi_motion(&mut self, _motion: ViMotion) -> bool {
        false
    }

    fn toggle_vi_selection(&mut self, _kind: SelectionKind) -> bool {
        false
    }

    fn vi_cursor(&self) -> Option<ViCursor> {
        None
    }
}

/// `MasterPty` whose `resize` always fails.
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

/// `MasterPty` recording every `PtySize` handed to `resize`, so a caller
/// can assert the exact struct that was forwarded.
///
/// It also answers `get_size` with whatever it was last resized to, so
/// a caller that writes a size and reads it back sees what a real
/// master would.
#[cfg(any(test, feature = "test-support"))]
#[derive(Debug)]
pub(crate) struct RecordingMaster {
    calls: Arc<Mutex<Vec<PtySize>>>,
    size: Mutex<PtySize>,
}

#[cfg(any(test, feature = "test-support"))]
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

    /// Builds the fake at `size` with zero pixel dimensions.
    pub(crate) fn at(size: GridSize) -> (Self, Arc<Mutex<Vec<PtySize>>>) {
        Self::new(CellPixels::default().pty_size(size))
    }
}

#[cfg(any(test, feature = "test-support"))]
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
