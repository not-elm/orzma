//! Terminal emulation for orzma.
//!
//! [`prelude`] gathers the vocabulary; the crate root defines [`Vt`],
//! the protocol between a self-contained terminal emulator and its
//! owner, and [`OrzmaVt`], the implementation of that protocol.

use crate::{
    device::DeviceState,
    device::modes::VtModes,
    frame::{Frame, FrameTracker},
    interpreter::Interpreter,
    placement::{InstanceId, PlacementSize},
    screen::grid::GridSize,
    screen::grid::coords::{GridColumn, GridPoint, ScreenLine},
    screen::selection::{CellSide, SelectionKind},
    screen::viewport::{DisplayOffset, Scroll},
};
use std::path::PathBuf;

mod device;
mod frame;
mod hyperlink;
mod interpreter;
mod placement;
mod screen;
mod vi;

/// The crate's vocabulary, gathered for downstream consumers.
///
/// A consumer imports the terminal types from here rather than from the
/// private modules that declare them, so the module tree stays free to
/// move a type without breaking anyone.
pub mod prelude {
    pub use crate::device::color::{Color, Palette, Rgb};
    pub use crate::device::modes::{
        KeypadMode, MouseEncoding, MouseTracking, ScreenKind, TextCursorEnable, VtModes,
    };
    pub use crate::frame::{DirtyRow, Frame};
    pub use crate::hyperlink::{Hyperlink, HyperlinkId, HyperlinkUri, is_allowed};
    pub use crate::placement::{AnchoredPlacement, InstanceId, MAX_COLS, MAX_ROWS, PlacementSize};
    pub use crate::screen::cursor::{CURSOR_VISIBLE_BIT, Cursor, CursorShape};
    pub use crate::screen::grid::GridSize;
    pub use crate::screen::grid::coords::{GridColumn, GridLine, GridPoint, ScreenLine};
    pub use crate::screen::grid::row::Row;
    pub use crate::screen::grid::run::{Run, Style};
    pub use crate::screen::selection::{
        CellSide, SelectionGeometry, SelectionKind, SelectionRange,
    };
    pub use crate::screen::viewport::{DisplayOffset, Scroll, ViewportLine};
    pub use crate::vi::{ViCursor, ViModeSwitch};
    pub use crate::{InterpretOutput, OrzmaVt, ResizeChanged, Vt, VtSignal};
}

/// The terminal-emulation contract `OrzmaTty` drives and the host
/// observes.
///
/// An implementor is a complete VT: it interprets the PTY stream, owns
/// the grid and scrollback, tracks damage, and builds frames. The
/// trait has no constructor — a concrete VT is built with its own
/// configuration and injected; spawn geometry arrives via
/// [`Vt::resize`]. Vi mode arrives later.
///
/// The read surface exposes no per-cell seam: cell-level host features
/// (e.g. hyperlink hover) resolve against the emitted
/// [`crate::prelude::Row`] / [`crate::prelude::Run`] data, and the only
/// text read, [`Vt::selection_text`], is a derived value, so the VT's
/// storage cell never leaves the crate.
///
/// # Invariants
///
/// - Every operation that can strand a placement names it in its own
///   result: [`Vt::interpret`] names it in [`InterpretOutput::signals`],
///   and [`Vt::resize`] names it in [`ResizeChanged::evicted`]. The only
///   other removal is the host-driven [`Vt::remove_placements`], which
///   reports nothing because the caller already named the ids. There
///   is no sweep for the owner to run.
/// - An owner that forwards [`InterpretOutput::signals`] and
///   [`ResizeChanged::evicted`] before it requests the next frame
///   delivers every eviction no later than the first frame that
///   reflects it; the VT does not promise that the two arrive in the
///   same batch.
pub trait Vt {
    /// Interprets one PTY chunk, staging its damage internally and
    /// returning everything else it produced.
    ///
    /// An empty chunk returns [`InterpretOutput::default`].
    ///
    /// # Invariants
    ///
    /// - [`InterpretOutput::signals`] keeps the order its own doc
    ///   states: parser-raised signals first, the chunk-end eviction last.
    /// - [`InterpretOutput::replies`] must be written back to the PTY.
    ///
    /// # Webview placements
    ///
    /// An APC webview `mount` the VT accepts becomes a
    /// [`VtSignal::WebviewMount`] carrying the [`InstanceId`] the mount
    /// named; one the placement cap refuses becomes a
    /// [`VtSignal::WebviewMountRejected`] instead, which registers
    /// nothing. The VT owns the placement table and projects every
    /// placement into [`Frame::placements`] on each emit; a mount, unmount,
    /// eviction, or projected-geometry change always raises the chunk
    /// liveness, so the frame carrying the new list is guaranteed to
    /// follow. Evictions the VT performs on its own authority (history
    /// trim, reset, alternate-screen teardown) surface as
    /// [`VtSignal::WebviewEvicted`]. At any instant the live ids are unique.
    ///
    /// A placement projects only while the screen it was mounted on is
    /// active: while the alternate screen is shown, primary-screen
    /// placements are omitted from the emitted lists (hidden, not
    /// evicted). Returning to the primary screen tears the alternate
    /// screen's placements down instead, naming them in that chunk's
    /// [`VtSignal::WebviewEvicted`]. A re-issued `mount` for a live
    /// instance updates that placement in place — the id does not
    /// change, and nothing is named by [`VtSignal::WebviewEvicted`].
    fn interpret(&mut self, chunk: &[u8]) -> InterpretOutput;

    /// Builds the frame for the staged damage and section diffs,
    /// consuming the staged damage; `None` when nothing observable
    /// changed — no rows staged and every diffed section equal to its
    /// last-emitted value.
    ///
    /// # Invariants
    ///
    /// - The first emitted frame carries every viewport row, as does
    ///   every frame after a viewport-basis change (resize, offset,
    ///   alternate-screen flip), because every basis change stages full
    ///   damage. The emit-time offset diff is only a liveness backstop:
    ///   it guarantees such a frame is emitted, not that it carries
    ///   rows.
    /// - A frame's placements and display offset describe the same
    ///   instant as its rows.
    fn frame(&mut self) -> Option<Frame>;

    /// Registers a host-driven mount anchored at the visible cell (`row`,
    /// `column`) of the active screen; `true` when the placement was
    /// registered, `false` when the cell lies outside the grid or the
    /// per-terminal cap is full.
    ///
    /// This is the control-socket counterpart of the APC `mount`, for PTYs
    /// that drop APC (ConPTY). The caller raises
    /// [`VtSignal::WebviewMount`] or [`VtSignal::WebviewMountRejected`]
    /// itself from the returned verdict. Like [`Vt::remove_placements`], it
    /// stages no row damage: the changed placement list alone carries the
    /// next frame.
    fn mount_placement_at(
        &mut self,
        row: ScreenLine,
        column: GridColumn,
        size: PlacementSize,
        instance: InstanceId,
    ) -> bool;

    /// Removes the placements the host names, on either screen; returns
    /// whether anything went.
    ///
    /// This is the control plane's entry point, used when a registration
    /// is released, its connection drops, or a mount the host refuses
    /// left a reservation behind. The VT cannot know any of those facts —
    /// they live on the control socket — so without this the placements
    /// keep a cap slot until their anchor scrolls out of history.
    ///
    /// # Invariants
    ///
    /// Unlike [`Vt::resize`], this stages no row damage. A placement list
    /// is an emit-time diffed section, so a changed list is enough to
    /// guarantee the frame that carries it; staging rows here would
    /// repaint the whole viewport on every release.
    ///
    /// No [`VtSignal::WebviewEvicted`] is raised: the caller already
    /// knows the ids and drops its own entities in the same pass, so a
    /// signal would hand it back its own removal.
    fn remove_placements(&mut self, instances: &[InstanceId]) -> bool;

    /// Resizes the grid, truncating rather than reflowing; `None` when
    /// the dimensions did not change. Only a real change stages (full)
    /// damage.
    ///
    /// # Invariants
    ///
    /// Both axes are nonzero; degenerate sizes are rejected by the
    /// caller.
    ///
    /// The placements the resize strands are named in the returned
    /// [`ResizeChanged::evicted`] and are already gone from the VT.
    #[must_use = "the evicted placements must reach the owner's signal queue"]
    fn resize(&mut self, size: GridSize) -> Option<ResizeChanged>;

    /// Applies the viewport motion; returns whether the viewport
    /// moved. Only a real move stages (full) damage.
    fn scroll(&mut self, scroll: Scroll) -> bool;

    /// Anchors a new selection at `cell`, replacing any active one;
    /// returns whether the selection state changed. A cell outside the
    /// grid (a line already evicted from history, or a column past the
    /// width) is rejected, leaving the current selection untouched.
    ///
    /// The return value reports the stored state, not the projection:
    /// a start whose projection is empty still returns `true`, and the
    /// emit-time diff decides on its own whether a frame is owed.
    fn start_selection(&mut self, cell: GridPoint, side: CellSide, kind: SelectionKind) -> bool;

    /// Moves the active selection's moving end to `cell`; returns
    /// whether the moving end changed. A no-op returning `false` when
    /// there is no active selection or `cell` is outside the grid.
    ///
    /// Two cells naming the same boundary — the right half of one and
    /// the left half of the next — are the same moving end.
    fn extend_selection(&mut self, cell: GridPoint, side: CellSide) -> bool;

    /// Drops the active selection; returns whether there was one.
    ///
    /// A selection whose rows have left the ring still counts: it holds
    /// state even though it projects nothing.
    fn clear_selection(&mut self) -> bool;

    /// The text the active selection covers; `None` exactly when
    /// [`Frame::selection`] would be `None` — no selection, an empty
    /// span, or an endpoint whose line has left the ring.
    fn selection_text(&self) -> Option<String>;

    /// Grid dimensions in cells.
    fn grid_size(&self) -> GridSize;

    /// Number of scrollback rows the viewport sits above the live tail.
    fn display_offset(&self) -> DisplayOffset;

    /// Returns `true` when the viewport is pinned to the live tail.
    #[inline]
    fn is_at_live_tail(&self) -> bool {
        self.display_offset() == DisplayOffset(0)
    }

    /// Snapshot of the device-wide DECSET / DECRST modes.
    fn modes(&self) -> VtModes;
}

/// Everything one [`Vt::interpret`] call produced besides the staged
/// damage.
#[derive(Debug, Default)]
pub struct InterpretOutput {
    /// Whether this chunk produced anything frame-relevant — staged row
    /// damage, cursor motion, or a mutated frame-visible section — so
    /// the owner knows to open its coalesce window.
    pub damaged: bool,
    /// Out-of-band signals: the parser-raised ones in byte-stream order,
    /// then the chunk-end [`VtSignal::WebviewEvicted`] when the chunk
    /// stranded a placement.
    pub signals: Vec<VtSignal>,
    /// Reply bytes (DSR, DA, …) the owner must write back to the PTY.
    pub replies: Vec<u8>,
}

/// What a [`Vt::resize`] that changed the dimensions caused besides the
/// grid change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResizeChanged {
    /// The placements whose anchor row the resize dropped out of
    /// history; empty when every anchor survived.
    pub evicted: Vec<InstanceId>,
}

/// Out-of-band signal the VT raised.
///
/// A chunk hands the signals it produced to the owner in
/// [`InterpretOutput::signals`]. A resize reports the placements it
/// stranded as ids in [`ResizeChanged::evicted`], and the owner wraps
/// them with [`Self::evicted`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VtSignal {
    /// An audible bell has been requested; the consumer is responsible
    /// for audio output or visual feedback (e.g. a flash).
    Bell,
    /// The OS title string changed, either because the application set
    /// one (OSC 0 or OSC 2) or because `CSI 23 t` restored a saved one.
    /// An icon name (OSC 1) is ignored, because this terminal carries
    /// one title.
    Title(String),
    /// The OS title string returned to the host's default, either
    /// because `CSI 23 t` restored a saved absence or because `RIS`
    /// reset the terminal. An empty title is [`VtSignal::Title`] with an
    /// empty string, not this.
    ResetTitle,
    /// The application copied data to the system clipboard via OSC 52.
    Clipboard {
        /// The clipboard content that was copied.
        content: String,
    },
    /// A new current working directory reported via OSC 7.
    CurrentDir(PathBuf),
    /// A webview the PTY mounted inline, which the VT accepted and
    /// registered at the cursor anchor.
    WebviewMount {
        /// The host-minted instance this mount registered.
        instance: InstanceId,
        /// The cell rectangle the mount reserved.
        size: PlacementSize,
    },
    /// A mount the VT refused because the per-terminal placement cap was
    /// already full. Nothing was registered, so there is nothing for the
    /// consumer to place; it exists so a webview that never appears is
    /// diagnosable rather than silent.
    WebviewMountRejected {
        /// The instance the refused mount named.
        instance: InstanceId,
    },
    /// The placement the PTY unmounted, or — when `instance` is `None` —
    /// every placement on this terminal.
    WebviewUnmount {
        /// The instance to unmount; `None` unmounts every placement.
        instance: Option<InstanceId>,
    },
    /// Placements the VT dropped without the host naming them (history
    /// trim, reset, alternate-screen teardown, resize). Consumers despawn
    /// them by id; unknown ids are ignored. A remount's superseded id is
    /// never named here — supersession shows only as the geometry
    /// changing in the frame-carried placement lists.
    WebviewEvicted {
        /// The instances that were evicted.
        placements: Vec<InstanceId>,
    },
    /// Tracked `TermMode` flags that transitioned since the previous
    /// signal drain, as mode names (e.g. "alt-screen").
    ModeChange {
        /// Mode names that were enabled.
        added: Vec<&'static str>,
        /// Mode names that were disabled.
        removed: Vec<&'static str>,
    },
}

impl VtSignal {
    /// The eviction naming `placements`; `None` when there is nothing
    /// to name, so neither a sweep that found nothing, a resize that
    /// stranded nothing, nor a flip that tore nothing down wakes the
    /// owner.
    pub fn evicted(placements: Vec<InstanceId>) -> Option<Self> {
        (!placements.is_empty()).then_some(Self::WebviewEvicted { placements })
    }
}

/// The self-contained implementation of [`Vt`]: a byte interpreter, the
/// emulated device it writes to, and the frame tracker that turns the
/// staged damage into frames.
pub struct OrzmaVt {
    /// Byte decoding plus the CSI ?2026 synchronized-update buffer.
    interpreter: Interpreter,
    /// The emulated device: screens, modes, tabs, colors, title.
    device: DeviceState,
    /// The pending damage and the retained last-emitted values.
    tracker: FrameTracker,
}

impl OrzmaVt {
    /// Builds a terminal whose first frame carries every viewport row.
    ///
    /// # Invariants
    ///
    /// Both grid axes are nonzero; degenerate sizes are rejected by the
    /// caller (the same contract as [`Vt::resize`]).
    ///
    /// The tracker must come from `FrameTracker::new`: its seeded
    /// full damage is what makes the first frame carry every viewport
    /// row, so a constructor that starts from an empty ledger paints
    /// nothing until the first PTY output arrives.
    pub fn new(size: GridSize, max_history: usize) -> Self {
        Self {
            interpreter: Interpreter::default(),
            device: DeviceState::new(size, max_history),
            tracker: FrameTracker::new(),
        }
    }
}

impl Vt for OrzmaVt {
    fn interpret(&mut self, chunk: &[u8]) -> InterpretOutput {
        if chunk.is_empty() {
            return InterpretOutput::default();
        }
        let mut output = InterpretOutput::default();
        self.interpreter
            .parse(&mut output, &mut self.device, &mut self.tracker, chunk);
        output
    }

    fn frame(&mut self) -> Option<Frame> {
        self.tracker.emit(&self.device)
    }

    fn mount_placement_at(
        &mut self,
        row: ScreenLine,
        column: GridColumn,
        size: PlacementSize,
        instance: InstanceId,
    ) -> bool {
        self.device.mount_placement_at(row, column, size, instance)
    }

    fn remove_placements(&mut self, instances: &[InstanceId]) -> bool {
        self.device.remove_placements(instances)
    }

    fn resize(&mut self, size: GridSize) -> Option<ResizeChanged> {
        let damage = self.device.resize(size)?;
        self.tracker.stage(damage);
        Some(ResizeChanged {
            evicted: self.device.evict_lost_anchors(),
        })
    }

    fn scroll(&mut self, scroll: Scroll) -> bool {
        self.tracker.stage_if_changed(self.device.scroll(scroll))
    }

    fn start_selection(&mut self, cell: GridPoint, side: CellSide, kind: SelectionKind) -> bool {
        self.device
            .active_screen_mut()
            .start_selection(cell, side, kind)
    }

    fn extend_selection(&mut self, cell: GridPoint, side: CellSide) -> bool {
        self.device.active_screen_mut().extend_selection(cell, side)
    }

    fn clear_selection(&mut self) -> bool {
        self.device.active_screen_mut().clear_selection()
    }

    fn selection_text(&self) -> Option<String> {
        self.device.active_screen().selection_text()
    }

    fn grid_size(&self) -> GridSize {
        self.device.grid_size()
    }

    fn display_offset(&self) -> DisplayOffset {
        self.device.display_offset()
    }

    fn modes(&self) -> VtModes {
        self.device.modes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::placement::{InstanceId, MAX_PLACEMENTS, PlacementSize};
    use crate::screen::grid::coords::{GridColumn, GridLine, ScreenLine};
    use crate::screen::selection::{SelectionGeometry, SelectionRange};
    use crate::screen::viewport::ViewportLine;

    fn vt() -> OrzmaVt {
        OrzmaVt::new(GridSize { cols: 4, rows: 3 }, 10)
    }

    /// A 4×3 terminal whose rows read `abcd` / `efgh` / `ijkl`, with the
    /// bootstrap frame already drained.
    fn filled() -> OrzmaVt {
        let mut vt = vt();
        vt.interpret(b"abcd\r\nefgh\r\nijkl");
        vt.frame();
        vt
    }

    fn cell(line: i32, column: u16) -> GridPoint {
        GridPoint {
            line: GridLine(line),
            column: GridColumn(column),
        }
    }

    /// The selection the active screen projects right now, read without
    /// consuming a frame.
    fn projected(vt: &OrzmaVt) -> Option<SelectionRange> {
        vt.device.active_screen().selection_range()
    }

    /// Asserts that the next frame repaints no rows and carries
    /// `selection`, which is what an idle selection change owes.
    fn assert_rowless_frame(vt: &mut OrzmaVt, selection: Option<SelectionRange>) {
        let frame = vt.frame().expect("an idle selection change emits");
        assert!(frame.rows.is_empty());
        assert_eq!(frame.selection, selection);
    }

    fn range(start: (i32, u16), end: (i32, u16), geometry: SelectionGeometry) -> SelectionRange {
        SelectionRange {
            start: cell(start.0, start.1),
            end: cell(end.0, end.1),
            geometry,
        }
    }

    /// Asserts that a Lines start projects the anchored row from its first
    /// to its last column.
    ///
    /// Case: the user triple-clicks the middle row of the terminal.
    #[test]
    fn a_lines_start_projects_the_whole_row() {
        let mut vt = filled();
        assert!(vt.start_selection(cell(1, 2), CellSide::Left, SelectionKind::Lines));
        assert_eq!(
            projected(&vt),
            Some(range((1, 0), (1, 3), SelectionGeometry::Lines))
        );
    }

    /// Asserts that a start identical to the active selection reports no
    /// change and owes no frame.
    ///
    /// Case: the host re-fires the same start request for a repeated
    /// triple-click on the row that is already selected.
    #[test]
    fn an_identical_start_is_a_no_op() {
        let mut vt = filled();
        vt.start_selection(cell(1, 0), CellSide::Left, SelectionKind::Lines);
        vt.frame();
        assert!(!vt.start_selection(cell(1, 0), CellSide::Left, SelectionKind::Lines));
        assert!(vt.frame().is_none());
    }

    /// Asserts that a start on a line the ring does not hold is rejected and
    /// leaves the active selection untouched.
    ///
    /// Case: the press was hit-tested against a frame that still showed a
    /// history row, which the terminal has since trimmed away.
    #[test]
    fn a_start_on_a_line_outside_the_ring_is_rejected() {
        let mut vt = filled();
        vt.start_selection(cell(1, 0), CellSide::Left, SelectionKind::Lines);
        assert!(!vt.start_selection(cell(-1, 0), CellSide::Left, SelectionKind::Lines));
        assert_eq!(
            projected(&vt),
            Some(range((1, 0), (1, 3), SelectionGeometry::Lines))
        );
    }

    /// Asserts that a start whose column is past the grid width is rejected
    /// even for a Lines selection that would ignore the column.
    ///
    /// Case: a shrink resize lands between the host's hit test and the
    /// request reaching the terminal.
    #[test]
    fn a_start_past_the_last_column_is_rejected() {
        let mut vt = filled();
        vt.start_selection(cell(1, 0), CellSide::Left, SelectionKind::Lines);
        assert!(!vt.start_selection(cell(0, 4), CellSide::Left, SelectionKind::Lines));
        assert_eq!(
            projected(&vt),
            Some(range((1, 0), (1, 3), SelectionGeometry::Lines))
        );
    }

    /// Asserts that a start on a history line the ring still holds is
    /// accepted and projects there.
    ///
    /// Case: the user scrolls back and triple-clicks a row that has already
    /// left the live screen.
    #[test]
    fn a_start_on_a_history_line_is_accepted() {
        let mut vt = filled();
        vt.interpret(b"\r\n\r\n");
        assert!(vt.start_selection(cell(-1, 0), CellSide::Left, SelectionKind::Lines));
        assert_eq!(
            projected(&vt),
            Some(range((-1, 0), (-1, 3), SelectionGeometry::Lines))
        );
    }

    /// Asserts that a fresh Simple start returns `true` even though its
    /// empty projection leaves `frame()` with nothing to emit.
    ///
    /// Case: the user presses the mouse button on a cell and the host
    /// anchors a selection before the pointer has moved.
    #[test]
    fn a_fresh_simple_start_changes_state_but_projects_nothing() {
        let mut vt = filled();
        assert!(vt.start_selection(cell(0, 1), CellSide::Left, SelectionKind::Simple));
        assert_eq!(projected(&vt), None);
        assert!(vt.frame().is_none());
    }

    /// Asserts that clearing when nothing is selected reports no change and
    /// owes no frame.
    ///
    /// Case: the user clicks in the terminal with no selection active, and
    /// the host sends its usual clear.
    #[test]
    fn a_clear_without_a_selection_is_a_no_op() {
        let mut vt = filled();
        assert!(!vt.clear_selection());
        assert!(vt.frame().is_none());
    }

    /// Asserts that clearing on an idle terminal emits a frame that drops
    /// the selection and repaints no rows.
    ///
    /// Case: the user clicks elsewhere to dismiss a selection while the
    /// shell is quiet.
    #[test]
    fn an_idle_clear_emits_a_frame_without_the_selection() {
        let mut vt = filled();
        vt.start_selection(cell(0, 0), CellSide::Left, SelectionKind::Lines);
        vt.frame();
        assert!(vt.clear_selection());
        assert_rowless_frame(&mut vt, None);
    }

    /// Asserts that a second clear finds nothing to drop.
    ///
    /// Case: the host sends a clear on every click, and the user clicks
    /// twice after dismissing a selection.
    #[test]
    fn a_repeated_clear_is_a_no_op() {
        let mut vt = filled();
        vt.start_selection(cell(0, 0), CellSide::Left, SelectionKind::Lines);
        assert!(vt.clear_selection());
        assert!(!vt.clear_selection());
    }

    /// Asserts that a Lines start on an idle terminal emits a frame that
    /// carries the new range and repaints no rows.
    ///
    /// Case: the user triple-clicks a row while the shell is quiet.
    #[test]
    fn an_idle_start_emits_a_rowless_frame_carrying_the_selection() {
        let mut vt = filled();
        assert!(vt.start_selection(cell(0, 0), CellSide::Left, SelectionKind::Lines));
        assert_rowless_frame(
            &mut vt,
            Some(range((0, 0), (0, 3), SelectionGeometry::Lines)),
        );
    }

    /// Asserts that a start while a selection is active replaces it rather
    /// than extending or keeping it.
    ///
    /// Case: with two rows selected, the user triple-clicks a different row.
    #[test]
    fn a_start_replaces_the_active_selection() {
        let mut vt = filled();
        vt.start_selection(cell(0, 0), CellSide::Left, SelectionKind::Lines);
        vt.extend_selection(cell(1, 0), CellSide::Left);
        assert!(vt.start_selection(cell(2, 1), CellSide::Left, SelectionKind::Lines));
        assert_eq!(
            projected(&vt),
            Some(range((2, 0), (2, 3), SelectionGeometry::Lines))
        );
    }

    /// Asserts that an anchor on the right half of a cell starts the
    /// selection on the following cell.
    ///
    /// Case: the user presses on the right half of column 1 and drags to
    /// the left half of column 3.
    #[test]
    fn a_right_side_anchor_starts_on_the_next_boundary() {
        let mut vt = filled();
        vt.start_selection(cell(0, 1), CellSide::Right, SelectionKind::Simple);
        assert!(vt.extend_selection(cell(0, 3), CellSide::Left));
        assert_eq!(
            projected(&vt),
            Some(range((0, 2), (0, 2), SelectionGeometry::Linear))
        );
    }

    /// Asserts that an extend with no active selection reports no change
    /// and creates nothing.
    ///
    /// Case: a drag update arrives after a click elsewhere already cleared
    /// the selection.
    #[test]
    fn an_extend_without_a_selection_is_a_no_op() {
        let mut vt = filled();
        assert!(!vt.extend_selection(cell(0, 2), CellSide::Right));
        assert_eq!(projected(&vt), None);
        assert!(vt.frame().is_none());
    }

    /// Asserts that dragging the moving end to the right projects the cells
    /// from the anchor through the cell under the pointer.
    ///
    /// Case: the user presses on the first cell of a row and drags across
    /// two more.
    #[test]
    fn a_forward_extend_projects_the_span() {
        let mut vt = filled();
        vt.start_selection(cell(0, 0), CellSide::Left, SelectionKind::Simple);
        assert!(vt.extend_selection(cell(0, 2), CellSide::Right));
        assert_eq!(
            projected(&vt),
            Some(range((0, 0), (0, 2), SelectionGeometry::Linear))
        );
    }

    /// Asserts that extending an idle terminal's selection emits a frame
    /// that carries the new range and repaints no rows.
    ///
    /// Case: the shell is quiet while the user keeps dragging.
    #[test]
    fn an_idle_extend_emits_a_rowless_frame() {
        let mut vt = filled();
        vt.start_selection(cell(0, 0), CellSide::Left, SelectionKind::Simple);
        vt.extend_selection(cell(0, 2), CellSide::Right);
        vt.frame();
        assert!(vt.extend_selection(cell(0, 3), CellSide::Right));
        assert_rowless_frame(
            &mut vt,
            Some(range((0, 0), (0, 3), SelectionGeometry::Linear)),
        );
    }

    /// Asserts that dragging above and left of the anchor swaps the ends so
    /// the projected range still reads top-left to bottom-right.
    ///
    /// Case: the user presses in the middle of the second row and drags up
    /// into the first.
    #[test]
    fn a_backward_drag_normalizes_the_endpoints() {
        let mut vt = filled();
        vt.start_selection(cell(1, 2), CellSide::Left, SelectionKind::Simple);
        assert!(vt.extend_selection(cell(0, 1), CellSide::Left));
        assert_eq!(
            projected(&vt),
            Some(range((0, 1), (1, 1), SelectionGeometry::Linear))
        );
    }

    /// Asserts that an extend to the same cell boundary the moving end already
    /// occupies reports no change, even when the column and side differ.
    ///
    /// Case: the pointer crosses from the right half of one cell into the
    /// left half of the next without moving the boundary.
    #[test]
    fn an_extend_to_the_same_boundary_is_a_no_op() {
        let mut vt = filled();
        vt.start_selection(cell(0, 1), CellSide::Right, SelectionKind::Simple);
        vt.frame();
        assert!(!vt.extend_selection(cell(0, 2), CellSide::Left));
        assert!(vt.frame().is_none());
    }

    /// Asserts that an anchor on the right edge of a row begins the range on
    /// the first cell of the row below.
    ///
    /// Case: the user presses on the right half of the last column and drags
    /// down into the next row.
    #[test]
    fn a_right_edge_anchor_wraps_to_the_next_row() {
        let mut vt = filled();
        vt.start_selection(cell(0, 3), CellSide::Right, SelectionKind::Simple);
        assert!(vt.extend_selection(cell(1, 1), CellSide::Right));
        assert_eq!(
            projected(&vt),
            Some(range((1, 0), (1, 1), SelectionGeometry::Linear))
        );
    }

    /// Asserts that a moving end on the left edge of a row ends the range on
    /// the last cell of the row above.
    ///
    /// Case: the user drags down and lands on the left half of the first
    /// column of the next row.
    #[test]
    fn a_left_edge_end_wraps_to_the_previous_row() {
        let mut vt = filled();
        vt.start_selection(cell(0, 1), CellSide::Left, SelectionKind::Simple);
        assert!(vt.extend_selection(cell(1, 0), CellSide::Left));
        assert_eq!(
            projected(&vt),
            Some(range((0, 1), (0, 3), SelectionGeometry::Linear))
        );
    }

    /// Asserts that a range whose ends wrap past each other projects nothing.
    ///
    /// Case: the user presses on the right half of the last column and drags
    /// onto the left half of the first column of the next row.
    #[test]
    fn a_wrap_that_crosses_itself_is_empty() {
        let mut vt = filled();
        vt.start_selection(cell(0, 3), CellSide::Right, SelectionKind::Simple);
        assert!(vt.extend_selection(cell(1, 0), CellSide::Left));
        assert_eq!(projected(&vt), None);
    }

    /// Asserts that a Lines drag spans whole rows from the anchor's row to
    /// the pointer's row regardless of the columns involved.
    ///
    /// Case: the user triple-clicks a row and drags two rows down.
    #[test]
    fn a_lines_extend_takes_whole_rows() {
        let mut vt = filled();
        vt.start_selection(cell(0, 2), CellSide::Left, SelectionKind::Lines);
        assert!(vt.extend_selection(cell(2, 1), CellSide::Left));
        assert_eq!(
            projected(&vt),
            Some(range((0, 0), (2, 3), SelectionGeometry::Lines))
        );
    }

    /// Asserts that moving the end of a Lines selection within its row
    /// changes the state but owes no frame.
    ///
    /// Case: the user drags sideways inside the triple-clicked row.
    #[test]
    fn a_column_only_move_under_lines_changes_no_projection() {
        let mut vt = filled();
        vt.start_selection(cell(0, 2), CellSide::Left, SelectionKind::Lines);
        vt.frame();
        assert!(vt.extend_selection(cell(0, 3), CellSide::Right));
        assert!(vt.frame().is_none());
    }

    /// Asserts that an extend to a line the ring does not hold is rejected
    /// and leaves the moving end where it was.
    ///
    /// Case: the drag reaches above the viewport on a terminal with no
    /// scrollback yet.
    #[test]
    fn an_extend_to_a_line_outside_the_ring_is_rejected() {
        let mut vt = filled();
        vt.start_selection(cell(0, 0), CellSide::Left, SelectionKind::Simple);
        vt.extend_selection(cell(0, 2), CellSide::Right);
        assert!(!vt.extend_selection(cell(-1, 0), CellSide::Left));
        assert_eq!(
            projected(&vt),
            Some(range((0, 0), (0, 2), SelectionGeometry::Linear))
        );
    }

    /// Asserts that an extend whose column is past the grid width is
    /// rejected rather than folded onto the row's right edge.
    ///
    /// Case: a shrink resize lands between the host's hit test and the drag
    /// update reaching the terminal.
    #[test]
    fn an_extend_past_the_last_column_is_rejected() {
        let mut vt = filled();
        vt.start_selection(cell(0, 0), CellSide::Left, SelectionKind::Simple);
        vt.extend_selection(cell(0, 2), CellSide::Right);
        assert!(!vt.extend_selection(cell(0, 4), CellSide::Left));
        assert_eq!(
            projected(&vt),
            Some(range((0, 0), (0, 2), SelectionGeometry::Linear))
        );
    }

    /// Asserts that a drag into scrollback the ring still holds extends the
    /// selection there.
    ///
    /// Case: the user triple-clicks the top live row and drags up into the
    /// history the terminal has retained.
    #[test]
    fn an_extend_into_history_is_accepted() {
        let mut vt = filled();
        vt.interpret(b"\r\n\r\n");
        vt.start_selection(cell(0, 0), CellSide::Left, SelectionKind::Lines);
        assert!(vt.extend_selection(cell(-2, 0), CellSide::Left));
        assert_eq!(
            projected(&vt),
            Some(range((-2, 0), (0, 3), SelectionGeometry::Lines))
        );
    }

    /// Asserts that a selection stays on the rows it was made on when output
    /// pushes those rows into history.
    ///
    /// Case: the user has two lines selected when the shell prints two more.
    #[test]
    fn a_selection_follows_its_rows_into_history() {
        let mut vt = filled();
        vt.start_selection(cell(0, 0), CellSide::Left, SelectionKind::Lines);
        vt.extend_selection(cell(1, 0), CellSide::Left);
        vt.interpret(b"\r\n\r\n");
        assert_eq!(
            projected(&vt),
            Some(range((-2, 0), (-1, 3), SelectionGeometry::Lines))
        );
    }

    /// Asserts that a selection whose end lies past a shrunken width projects
    /// onto the new last column instead of past the grid.
    ///
    /// Case: the user has a full row selected and narrows the window.
    #[test]
    fn a_width_shrink_clamps_the_projected_column() {
        let mut vt = filled();
        vt.start_selection(cell(0, 0), CellSide::Left, SelectionKind::Simple);
        vt.extend_selection(cell(0, 3), CellSide::Right);
        assert!(vt.resize(GridSize { cols: 2, rows: 3 }).is_some());
        assert_eq!(
            projected(&vt),
            Some(range((0, 0), (0, 1), SelectionGeometry::Linear))
        );
    }

    /// Asserts that a drag update after a clear has no selection to extend.
    ///
    /// Case: a stale drag update arrives after the click that cleared the
    /// selection.
    #[test]
    fn an_extend_after_a_clear_is_a_no_op() {
        let mut vt = filled();
        vt.start_selection(cell(0, 0), CellSide::Left, SelectionKind::Lines);
        vt.clear_selection();
        assert!(!vt.extend_selection(cell(1, 0), CellSide::Left));
        assert_eq!(projected(&vt), None);
    }

    /// Asserts that a resize to the size the grid already has returns
    /// `None`, so it neither stages damage nor names anything.
    ///
    /// Case: the window manager re-sends the geometry the terminal
    /// already has after a focus change.
    #[test]
    fn a_same_size_resize_returns_none() {
        let mut vt = vt();
        assert_eq!(vt.resize(GridSize { cols: 4, rows: 3 }), None);
    }

    /// Asserts that a shrink names the placement whose anchor row it
    /// dropped out of history in its own result.
    ///
    /// Case: a webview is mounted on a short-scrollback terminal and
    /// the user drags the window shorter, pushing its anchor row past
    /// the history cap.
    #[test]
    fn a_shrink_names_the_placement_it_stranded() {
        let mut vt = OrzmaVt::new(GridSize { cols: 4, rows: 4 }, 0);
        let id = InstanceId(1);
        assert!(
            vt.device
                .mount_placement(PlacementSize { rows: 1, cols: 1 }, id)
        );
        vt.device.active_screen_mut().move_cursor_to(Some(4), None);
        assert_eq!(
            vt.resize(GridSize { cols: 4, rows: 2 }),
            Some(ResizeChanged { evicted: vec![id] })
        );
    }

    /// Asserts that a fresh terminal's first frame carries every
    /// viewport row.
    ///
    /// Case: a terminal spawns and the renderer has nothing on screen
    /// yet, so the shell's first prompt must arrive with the whole
    /// viewport behind it.
    #[test]
    fn the_first_frame_carries_every_viewport_row() {
        let mut vt = vt();
        let frame = vt.frame().expect("the seeded Full emits");
        assert_eq!(frame.rows.len(), 3);
    }

    /// Asserts that interpreting a printable chunk reports the chunk as
    /// damaged through the public entry point.
    ///
    /// Case: a shell echoes the first character of its prompt into a
    /// freshly spawned terminal.
    #[test]
    fn interpreting_a_printable_chunk_reports_damage() {
        let mut vt = vt();
        assert!(vt.interpret(b"a").damaged);
    }

    /// Asserts that emitting drains the staged damage and settles the
    /// diffs, so an immediate second poll emits nothing.
    ///
    /// Case: the host polls for a frame twice in one tick, and the
    /// second poll must not repaint what the first one already sent.
    #[test]
    fn an_emitted_frame_leaves_nothing_to_emit() {
        let mut vt = vt();
        vt.frame();
        assert!(vt.frame().is_none());
    }

    /// Asserts that a print's viewport-space damage survives the
    /// `Screen` → `Damage` → `Frame` seam without the row it names
    /// being confused with the screen-space row it was computed from.
    ///
    /// Case: a shell prints at the top of a fresh screen, that row
    /// scrolls into history, and the user has scrolled back to it by the
    /// time the frame for that print is finally emitted.
    #[test]
    fn a_staged_print_survives_the_composed_pipeline() {
        let mut vt = vt();
        vt.frame();
        let damage = vt.device.active_screen_mut().print('x');
        vt.tracker.stage_if_changed(damage);
        for _ in 0..3 {
            vt.device.active_screen_mut().line_feed();
        }
        vt.device
            .active_screen_mut()
            .set_display_offset(DisplayOffset(1));
        let frame = vt.frame().expect("staged row damage emits");
        assert_eq!(frame.rows[0].line, ViewportLine(0));
        assert_eq!(frame.rows[0].contents[0].text, "x   ");
    }

    /// Asserts that flipping screens replays the placement sections as
    /// `Some(list)` → `Some(empty)` → `Some(list)`, with `None` between
    /// unchanged emits.
    ///
    /// Case: a shell with a mounted webview opens a full-screen editor
    /// on the alternate screen and closes it again.
    #[test]
    fn a_screen_flip_replays_the_placement_list() {
        let mut vt = vt();
        vt.frame();
        assert!(
            vt.device
                .mount_placement(PlacementSize { rows: 2, cols: 4 }, InstanceId(1))
        );
        let mounted = vt.frame().expect("a placement change emits");
        assert_eq!(mounted.placements.as_ref().map(Vec::len), Some(1));

        vt.interpret(b"\x1b[?47h");
        let flipped = vt.frame().expect("a flip emits a full frame");
        assert_eq!(flipped.placements, Some(Vec::new()));
        assert_eq!(flipped.rows.len(), 3);

        vt.interpret(b"\x1b[?47l");
        let restored = vt.frame().expect("the flip back emits");
        assert_eq!(restored.placements.as_ref().map(Vec::len), Some(1));
    }

    /// Asserts that returning to a scrolled-back primary screen repaints
    /// every viewport row at the offset the user left it at.
    ///
    /// Case: the user scrolls the shell back two lines, runs a
    /// full-screen program, and exits it.
    #[test]
    fn a_flip_back_keeps_the_primary_screens_scroll_position() {
        let mut vt = vt();
        vt.interpret(b"1\r\n2\r\n3\r\n4\r\n5");
        assert!(vt.scroll(Scroll::Delta(2)));
        vt.frame();

        vt.interpret(b"\x1b[?1049h");
        let entered = vt.frame().expect("a flip emits");
        assert_eq!(entered.display_offset, DisplayOffset(0));
        assert_eq!(entered.rows.len(), 3);

        vt.interpret(b"\x1b[?1049l");
        let returned = vt.frame().expect("the flip back emits");
        assert_eq!(returned.rows.len(), 3);
        assert_eq!(returned.display_offset, DisplayOffset(2));
    }

    /// Asserts that a return from the alternate screen after the window
    /// grew seats the restored cursor on the prompt row the growth moved
    /// down, not on the reclaimed rows above it.
    ///
    /// Case: the shell prompt sits on the bottom row, the user runs a
    /// full-screen program, maximises the window while it is up, and
    /// quits it.
    #[test]
    fn a_flip_back_after_a_growth_restores_the_cursor_onto_its_row() {
        let mut vt = vt();
        vt.interpret(b"1\r\n2\r\n3\r\n4\r\n5");
        vt.interpret(b"\x1b[?1049h");
        assert!(vt.resize(GridSize { cols: 4, rows: 5 }).is_some());
        vt.interpret(b"\x1b[?1049l");
        let returned = vt.frame().expect("the flip back emits");
        assert_eq!(returned.rows[4].contents[0].text, "5   ");
        assert_eq!(returned.cursor.point.line, GridLine(4));
    }

    /// Asserts that a host-driven removal drops the named placements,
    /// reports whether anything went, and — unlike a resize — stages no
    /// row damage of its own.
    ///
    /// Case: a program's control-plane connection drops while two of its
    /// views are mounted, and the host clears what the registrations had
    /// reserved.
    #[test]
    fn a_host_removal_drops_the_named_placements_without_staging_rows() {
        let a: InstanceId = "3f5a9c02d1e84b7690ab3cde12f45678"
            .parse()
            .expect("valid id");
        let b: InstanceId = "81b4e77c05a3492fd6180e29ba735fc1"
            .parse()
            .expect("valid id");
        let mut vt = OrzmaVt::new(GridSize { cols: 80, rows: 24 }, 100);
        vt.interpret(format!("\x1b_Omount;n={a},r=4,c=8\x1b\\").as_bytes());
        vt.interpret(format!("\x1b_Omount;n={b},r=4,c=8\x1b\\").as_bytes());
        vt.frame().expect("the mounts damage the chunk");

        assert!(vt.remove_placements(&[a]));
        let frame = vt.frame().expect("the placement list changed");
        assert!(frame.rows.is_empty(), "a removal stages no row damage");
        let placements = frame.placements.expect("the list changed");
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].id, b);

        assert!(
            !vt.remove_placements(&[a]),
            "a second removal names nothing"
        );
    }

    /// Asserts that a host-driven mount anchors at the named visible cell
    /// and the next frame carries the placement there, staging no row
    /// damage and needing no interpreted bytes.
    ///
    /// Case: an SDK program in a Windows pane, where ConPTY drops the APC
    /// verb, mounts its view over the control socket at row 1, column 2.
    #[test]
    fn a_host_mount_anchors_at_the_named_cell_and_emits_a_frame() {
        let id: InstanceId = "3f5a9c02d1e84b7690ab3cde12f45678"
            .parse()
            .expect("valid id");
        let size = PlacementSize { rows: 4, cols: 8 };
        let mut vt = OrzmaVt::new(GridSize { cols: 80, rows: 24 }, 100);
        let _ = vt.frame();

        assert!(vt.mount_placement_at(ScreenLine(1), GridColumn(2), size, id));
        let frame = vt.frame().expect("the placement list changed");
        assert!(frame.rows.is_empty(), "a host mount stages no row damage");
        let placements = frame.placements.expect("the list changed");
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].id, id);
        assert_eq!(placements[0].point.line, GridLine(1));
        assert_eq!(placements[0].point.column, GridColumn(2));
        assert_eq!(placements[0].size, size);
    }

    /// Asserts that a host-driven mount naming a cell outside the grid is
    /// rejected and leaves a live placement under the same id untouched.
    ///
    /// Case: an SDK program re-mounts its view after the window shrank,
    /// naming a row the smaller grid no longer has.
    #[test]
    fn a_host_mount_outside_the_grid_is_rejected_without_touching_the_live_placement() {
        let id: InstanceId = "3f5a9c02d1e84b7690ab3cde12f45678"
            .parse()
            .expect("valid id");
        let size = PlacementSize { rows: 4, cols: 8 };
        let mut vt = OrzmaVt::new(GridSize { cols: 80, rows: 24 }, 100);
        assert!(vt.mount_placement_at(ScreenLine(1), GridColumn(2), size, id));
        let _ = vt.frame();

        assert!(!vt.mount_placement_at(ScreenLine(24), GridColumn(2), size, id));
        assert!(!vt.mount_placement_at(ScreenLine(1), GridColumn(80), size, id));
        assert!(vt.frame().is_none(), "a rejected mount changes nothing");
        assert!(
            vt.remove_placements(&[id]),
            "the earlier placement is still live"
        );
    }

    /// Asserts that a host-driven mount is rejected once the per-terminal
    /// cap is full, the same as an APC mount.
    ///
    /// Case: a program mounts one placement more than the terminal can
    /// display.
    #[test]
    fn a_host_mount_past_the_cap_is_rejected() {
        let size = PlacementSize { rows: 1, cols: 1 };
        let mut vt = OrzmaVt::new(GridSize { cols: 80, rows: 24 }, 100);
        for n in 0..MAX_PLACEMENTS {
            assert!(vt.mount_placement_at(
                ScreenLine(0),
                GridColumn(0),
                size,
                InstanceId(n as u128 + 1)
            ));
        }
        assert!(!vt.mount_placement_at(ScreenLine(0), GridColumn(0), size, InstanceId(u128::MAX)));
    }

    /// Asserts that a host-driven mount issued while the alternate screen
    /// is active lands on the alternate screen, so returning to the
    /// primary screen evicts it exactly as an APC mount's placement.
    ///
    /// Case: orzmd, a full-screen program, mounts its view over the socket
    /// and later exits back to the shell.
    #[test]
    fn a_host_mount_during_the_alternate_screen_lands_on_it() {
        let id: InstanceId = "3f5a9c02d1e84b7690ab3cde12f45678"
            .parse()
            .expect("valid id");
        let mut vt = OrzmaVt::new(GridSize { cols: 80, rows: 24 }, 100);
        vt.interpret(b"\x1b[?1049h");
        let _ = vt.frame();

        assert!(vt.mount_placement_at(
            ScreenLine(1),
            GridColumn(2),
            PlacementSize { rows: 4, cols: 8 },
            id
        ));
        let frame = vt.frame().expect("the placement list changed");
        assert_eq!(frame.placements.expect("the list changed")[0].id, id);

        let out = vt.interpret(b"\x1b[?1049l");
        assert!(
            out.signals.contains(&VtSignal::WebviewEvicted {
                placements: vec![id]
            }),
            "leaving the alternate screen evicts the placement, got {:?}",
            out.signals
        );
    }

    /// Asserts that a primary-screen selection is hidden while the alternate
    /// screen is shown and comes back unchanged on return.
    ///
    /// Case: the user selects a shell line, opens a full-screen editor, and
    /// quits it.
    #[test]
    fn a_primary_selection_hides_behind_the_alternate_screen() {
        let mut vt = filled();
        vt.start_selection(cell(0, 0), CellSide::Left, SelectionKind::Lines);
        vt.interpret(b"\x1b[?1049h");
        assert_eq!(projected(&vt), None);
        vt.interpret(b"\x1b[?1049l");
        assert_eq!(
            projected(&vt),
            Some(range((0, 0), (0, 3), SelectionGeometry::Lines))
        );
    }

    /// Asserts that a selection made on the alternate screen is dropped when
    /// the terminal returns to the primary screen, so a later alternate
    /// session does not inherit it.
    ///
    /// Case: the user selects a line inside a pager, quits it, and opens
    /// another full-screen program.
    #[test]
    fn an_alternate_selection_is_discarded_on_return() {
        let mut vt = filled();
        vt.interpret(b"\x1b[?1049h");
        vt.start_selection(cell(0, 0), CellSide::Left, SelectionKind::Lines);
        vt.interpret(b"\x1b[?1049l");
        assert_eq!(projected(&vt), None);
        vt.interpret(b"\x1b[?1049h");
        assert_eq!(projected(&vt), None);
    }

    /// Asserts that a reset on an otherwise blank grid still marks the chunk
    /// damaged and emits a frame without the selection.
    ///
    /// Case: a blank terminal has a line selected when a program issues RIS.
    #[test]
    fn a_reset_clears_the_selection_and_reports_it() {
        let mut vt = vt();
        vt.frame();
        vt.start_selection(cell(0, 0), CellSide::Left, SelectionKind::Lines);
        vt.frame();
        let output = vt.interpret(b"\x1bc");
        assert!(output.damaged);
        assert_eq!(vt.frame().expect("the reset emits").selection, None);
    }

    /// Asserts that a selection whose row has left the ring still counts as
    /// present for `clear_selection`, even though it no longer projects.
    ///
    /// Case: on a terminal with no scrollback the user selects the top row,
    /// the shell scrolls it away, and the user clicks to dismiss.
    #[test]
    fn a_clear_of_a_dead_selection_still_reports_true() {
        let mut vt = OrzmaVt::new(GridSize { cols: 4, rows: 3 }, 0);
        vt.interpret(b"abcd\r\nefgh\r\nijkl");
        vt.start_selection(cell(0, 0), CellSide::Left, SelectionKind::Lines);
        vt.interpret(b"\r\n");
        assert_eq!(projected(&vt), None);
        assert!(vt.clear_selection());
    }

    /// Asserts that a terminal with no selection has no text to copy.
    ///
    /// Case: the user presses the copy shortcut without having selected
    /// anything.
    #[test]
    fn no_selection_yields_no_text() {
        let vt = filled();
        assert_eq!(vt.selection_text(), None);
    }

    /// Asserts that a single-row Simple selection reads exactly the cells
    /// between its two boundaries.
    ///
    /// Case: the user drags across the middle two characters of a word.
    #[test]
    fn a_simple_span_reads_the_cells_between_its_ends() {
        let mut vt = filled();
        vt.start_selection(cell(0, 1), CellSide::Left, SelectionKind::Simple);
        vt.extend_selection(cell(0, 2), CellSide::Right);
        assert_eq!(vt.selection_text().as_deref(), Some("bc"));
    }

    /// Asserts that a Simple selection spanning three rows takes the tail of
    /// the first, the whole middle row, and the head of the last, joined by
    /// newlines with none at the end.
    ///
    /// Case: the user drags from the middle of one line down into the
    /// middle of the line two below it.
    #[test]
    fn a_multi_row_span_joins_rows_with_newlines() {
        let mut vt = filled();
        vt.start_selection(cell(0, 2), CellSide::Left, SelectionKind::Simple);
        vt.extend_selection(cell(2, 1), CellSide::Right);
        assert_eq!(vt.selection_text().as_deref(), Some("cd\nefgh\nij"));
    }

    /// Asserts that the blank cells past a row's last printed character are
    /// not copied.
    ///
    /// Case: the user selects two short lines on a wide terminal.
    #[test]
    fn trailing_blanks_are_trimmed_per_row() {
        let mut vt = vt();
        vt.interpret(b"ab\r\ncd");
        vt.frame();
        vt.start_selection(cell(0, 0), CellSide::Left, SelectionKind::Lines);
        vt.extend_selection(cell(1, 0), CellSide::Left);
        assert_eq!(vt.selection_text().as_deref(), Some("ab\ncd"));
    }

    /// Asserts that a Lines selection copies whole rows regardless of the
    /// columns the drag touched.
    ///
    /// Case: the user triple-clicks a line and drags into the next.
    #[test]
    fn a_lines_selection_reads_whole_rows() {
        let mut vt = filled();
        vt.start_selection(cell(0, 2), CellSide::Left, SelectionKind::Lines);
        vt.extend_selection(cell(1, 1), CellSide::Left);
        assert_eq!(vt.selection_text().as_deref(), Some("abcd\nefgh"));
    }

    /// Asserts that a selection whose two ends sit on the same boundary
    /// yields no text, matching the frame that paints nothing.
    ///
    /// Case: the user presses the mouse button and releases it without
    /// crossing a cell.
    #[test]
    fn an_empty_simple_selection_yields_no_text() {
        let mut vt = filled();
        vt.start_selection(cell(0, 1), CellSide::Left, SelectionKind::Simple);
        assert_eq!(vt.selection_text(), None);
    }

    /// Asserts that a selection whose row has been recycled out of the ring
    /// yields no text rather than the row now in its place.
    ///
    /// Case: on a terminal with no scrollback the user selects the top row
    /// and the shell scrolls it away before the copy.
    #[test]
    fn a_selection_whose_line_left_the_ring_yields_no_text() {
        let mut vt = OrzmaVt::new(GridSize { cols: 4, rows: 3 }, 0);
        vt.interpret(b"abcd\r\nefgh\r\nijkl");
        vt.frame();
        vt.start_selection(cell(0, 0), CellSide::Left, SelectionKind::Lines);
        vt.interpret(b"\r\n");
        assert_eq!(vt.selection_text(), None);
    }

    /// Asserts that the copied text is the row the user selected, not the
    /// row that has since scrolled into its screen position.
    ///
    /// Case: the user selects a line and the shell prints two more before
    /// the copy shortcut lands.
    #[test]
    fn the_text_follows_the_rows_after_a_scroll() {
        let mut vt = filled();
        vt.start_selection(cell(0, 0), CellSide::Left, SelectionKind::Lines);
        vt.interpret(b"\r\n\r\n");
        assert_eq!(vt.selection_text().as_deref(), Some("abcd"));
    }

    /// Asserts that a primary-screen selection yields no text while the
    /// alternate screen is shown and its text again once it is back.
    ///
    /// Case: the user selects a shell line, opens a pager, presses copy
    /// inside it, quits, and presses copy again.
    #[test]
    fn a_hidden_primary_selection_yields_no_text() {
        let mut vt = filled();
        vt.start_selection(cell(0, 0), CellSide::Left, SelectionKind::Lines);
        vt.interpret(b"\x1b[?1049h");
        assert_eq!(vt.selection_text(), None);
        vt.interpret(b"\x1b[?1049l");
        assert_eq!(vt.selection_text().as_deref(), Some("abcd"));
    }
}
