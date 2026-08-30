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
    interpreter::apc::WebviewApcVerb,
    placement::PlacementId,
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
mod selection;
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
    pub use crate::interpreter::apc::WebviewApcVerb;
    pub use crate::placement::{AnchoredPlacement, PlacementId, PlacementSize};
    pub use crate::screen::cursor::{CURSOR_VISIBLE_BIT, Cursor, CursorShape};
    pub use crate::screen::grid::GridSize;
    pub use crate::screen::grid::coords::{GridColumn, GridLine, GridPoint, ScreenLine};
    pub use crate::screen::grid::row::Row;
    pub use crate::screen::grid::run::{Run, Style};
    pub use crate::screen::viewport::{DisplayOffset, Scroll, ViewportLine};
    pub use crate::selection::{CellSide, SelectionGeometry, SelectionKind, SelectionRange};
    pub use crate::vi::{ViCursor, ViModeSwitch};
    pub use crate::{InterpretOutput, OrzmaVt, Vt, VtSignal};
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
pub trait Vt {
    /// Interprets one PTY chunk, staging its damage internally and
    /// returning everything else it produced.
    ///
    /// An empty chunk returns [`InterpretOutput::default`].
    ///
    /// # Invariants
    ///
    /// - [`InterpretOutput::signals`] preserves byte-stream order.
    /// - [`InterpretOutput::replies`] must be written back to the PTY.
    ///
    /// # Webview placements
    ///
    /// An APC webview `mount` becomes a [`VtSignal::WebviewApc`] whose
    /// [`PlacementId`] the VT mints itself; `placement: None` is a policy
    /// rejection. The VT owns the placement table and projects every
    /// placement into [`Frame::placements`] on each emit; a mount, unmount,
    /// eviction, or projected-geometry change always raises the chunk
    /// liveness, so the frame carrying the new list is guaranteed to
    /// follow. Evictions the VT performs on its own authority (history
    /// trim, alternate-screen teardown) surface as
    /// [`VtSignal::WebviewEvicted`]. Ids are never reused within a session.
    ///
    /// A placement projects only while the screen it was mounted on is
    /// active: while the alternate screen is shown, primary-screen
    /// placements are omitted from the emitted lists (hidden, not
    /// evicted), and the reverse on returning to the primary screen. A
    /// re-issued `mount` for a live `(view_id, instance)` registers a
    /// successor under a fresh id; the superseded id simply stops
    /// being listed and is never named by
    /// [`VtSignal::WebviewEvicted`].
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

    /// Evicts every placement whose anchor row no longer resolves and
    /// names them, so the owner can despawn what the VT destroyed.
    ///
    /// # Invariants
    ///
    /// The owner calls this before draining its signal queue and before
    /// asking for a frame, so the eviction still reaches that frame's
    /// placement list — a sweep after the damage ledger drained would
    /// reach no frame at all.
    ///
    /// A placement is named once. The sweep removes what it names, so a
    /// second sweep with nothing further lost raises nothing.
    fn sweep_evictions(&mut self) -> Vec<VtSignal>;

    /// Resizes the grid, truncating rather than reflowing; returns
    /// whether the dimensions changed. Only a real change stages (full)
    /// damage.
    ///
    /// # Invariants
    ///
    /// Both axes are nonzero; degenerate sizes are rejected by the
    /// caller.
    ///
    /// Placements the resize strands are not reported here. The owner's
    /// next [`Vt::sweep_evictions`] names them.
    fn resize(&mut self, size: GridSize) -> bool;

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
    /// Out-of-band signals, in byte-stream order.
    pub signals: Vec<VtSignal>,
    /// Reply bytes (DSR, DA, …) the owner must write back to the PTY.
    pub replies: Vec<u8>,
}

/// Out-of-band signal the VT raised, handed to the owner in
/// [`InterpretOutput::signals`] when it was parsed from the byte
/// stream, or returned from [`Vt::sweep_evictions`] when the VT raised
/// it on its own authority.
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
    /// An APC-driven webview mount/unmount request from the PTY.
    /// The placement is the VT-minted id, `Some` only for a `Mount` the
    /// VT accepted and registered; `None` is a policy rejection the
    /// consumer drops.
    WebviewApc {
        /// The mount or unmount verb and associated metadata.
        verb: WebviewApcVerb,
        /// The unique identifier for this placement, minted by the VT.
        placement: Option<PlacementId>,
    },
    /// Placements the VT evicted on its own authority (history trim,
    /// alternate-screen teardown). Consumers despawn them by id;
    /// unknown ids are ignored. A remount's superseded id is never
    /// named here — supersession shows only as the id vanishing from
    /// the frame-carried placement lists.
    WebviewEvicted {
        /// The placement IDs that were evicted.
        placements: Vec<PlacementId>,
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

/// The self-contained implementation of [`Vt`].
///
/// The fields are wired; several methods are still stubs. The
/// components land one at a time, in the order
/// `docs/orzma_vt_internal_design.md` §7 sets out.
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

    fn sweep_evictions(&mut self) -> Vec<VtSignal> {
        let placements = self.device.evict_lost_anchors();
        if placements.is_empty() {
            return Vec::new();
        }
        vec![VtSignal::WebviewEvicted { placements }]
    }

    fn resize(&mut self, size: GridSize) -> bool {
        self.tracker.stage_if_changed(self.device.resize(size))
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
    use crate::device::modes::ScreenKind;
    use crate::frame::damage::DamageSpan;
    use crate::placement::PlacementSize;
    use crate::screen::viewport::ViewportLine;

    fn vt() -> OrzmaVt {
        OrzmaVt::new(GridSize { cols: 4, rows: 3 }, 10)
    }

    /// Asserts that a sweep with nothing to evict raises no signal.
    ///
    /// Case: the host pumps a terminal that has no webviews mounted,
    /// which is every pump on a plain shell session.
    #[test]
    fn a_sweep_with_nothing_lost_raises_no_signal() {
        let mut vt = vt();
        assert!(vt.sweep_evictions().is_empty());
    }

    /// Asserts that a sweep after a reset names every placement the
    /// reset stranded, in one signal.
    ///
    /// Case: a webview is mounted and the shell sends `RIS`, so the
    /// host must despawn it.
    #[test]
    fn a_sweep_after_a_reset_names_the_stranded_placements() {
        let mut vt = vt();
        let id = vt
            .device
            .mount_placement(PlacementSize { rows: 1, cols: 1 }, "v".to_string(), None)
            .expect("a mount under the cap is accepted");
        vt.device.reset();
        assert_eq!(
            vt.sweep_evictions(),
            vec![VtSignal::WebviewEvicted {
                placements: vec![id]
            }]
        );
    }

    /// Asserts that a sweep after a shrink names the placement whose
    /// anchor row the shrink dropped out of history.
    ///
    /// Case: a webview is mounted on a short-scrollback terminal and
    /// the user drags the window shorter, pushing its anchor row past
    /// the history cap.
    #[test]
    fn a_sweep_after_a_shrink_names_the_placement_it_stranded() {
        let mut vt = OrzmaVt::new(GridSize { cols: 4, rows: 4 }, 0);
        let id = vt
            .device
            .mount_placement(PlacementSize { rows: 1, cols: 1 }, "v".to_string(), None)
            .expect("a mount under the cap is accepted");
        vt.device.active_screen_mut().move_cursor_to(Some(4), None);
        assert!(vt.resize(GridSize { cols: 4, rows: 2 }));
        assert_eq!(
            vt.sweep_evictions(),
            vec![VtSignal::WebviewEvicted {
                placements: vec![id]
            }]
        );
    }

    /// Asserts that a second sweep after the first raises nothing, so
    /// a per-pump sweep does not re-report what it already named.
    ///
    /// Case: the host pumps again on the frame after a reset despawned
    /// a webview.
    #[test]
    fn a_second_sweep_raises_nothing() {
        let mut vt = vt();
        vt.device
            .mount_placement(PlacementSize { rows: 1, cols: 1 }, "v".to_string(), None)
            .expect("a mount under the cap is accepted");
        vt.device.reset();
        assert_eq!(vt.sweep_evictions().len(), 1);
        assert!(vt.sweep_evictions().is_empty());
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
        vt.device
            .mount_placement(PlacementSize { rows: 2, cols: 4 }, "memo".to_string(), None)
            .expect("a mount under the cap is accepted");
        let mounted = vt.frame().expect("a placement change emits");
        assert_eq!(mounted.placements.as_ref().map(Vec::len), Some(1));

        vt.device.set_active_screen_for_test(ScreenKind::Alternate);
        vt.tracker.stage(DamageSpan::Full);
        let flipped = vt.frame().expect("a flip emits a full frame");
        assert_eq!(flipped.placements, Some(Vec::new()));
        assert_eq!(flipped.rows.len(), 3);

        vt.device.set_active_screen_for_test(ScreenKind::Primary);
        vt.tracker.stage(DamageSpan::Full);
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

        vt.interpret(b"\x1b[?1049l");
        let returned = vt.frame().expect("the flip back emits");
        assert_eq!(returned.rows.len(), 3);
        assert_eq!(returned.display_offset, DisplayOffset(2));
    }
}
