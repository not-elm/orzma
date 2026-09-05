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
    pub use crate::device::modes::{KeypadMode, MouseEncoding, MouseTracking, ScreenKind, VtModes};
    pub use crate::frame::{DirtyRow, Frame};
    pub use crate::hyperlink::{Hyperlink, HyperlinkId, HyperlinkUri, is_allowed};
    pub use crate::placement::{AnchoredPlacement, InstanceId, PlacementSize};
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
/// [`Vt::resize`]. Selection and vi mode arrive later as separate
/// capability traits.
///
/// The read surface is deliberately frame-granular: cell-level host
/// features (e.g. hyperlink hover) resolve against the emitted
/// [`crate::prelude::Row`] / [`crate::prelude::Run`] data, so the
/// trait exposes no per-cell read seam and the VT's storage cell
/// never leaves the crate.
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

    /// Grid dimensions in cells.
    fn grid_size(&self) -> GridSize;

    /// Number of scrollback rows the viewport sits above the live tail.
    fn display_offset(&self) -> DisplayOffset;

    /// Returns `true` when the viewport is pinned to the live tail.
    #[inline]
    fn is_at_live_tail(&self) -> bool {
        self.display_offset() == DisplayOffset(0)
    }

    /// Snapshot of the input-relevant terminal modes.
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
    use crate::placement::{InstanceId, PlacementSize};
    use crate::screen::grid::coords::GridLine;
    use crate::screen::viewport::ViewportLine;

    fn vt() -> OrzmaVt {
        OrzmaVt::new(GridSize { cols: 4, rows: 3 }, 10)
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
}
