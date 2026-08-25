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
    interpreter::apc::ApcWebviewVerb,
    placement::{PlacementId, PlacementStore},
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
    pub use crate::device::modes::{MouseEncoding, MouseTracking, ScreenKind, VtModes};
    pub use crate::frame::{DirtyRow, Frame};
    pub use crate::hyperlink::{Hyperlink, HyperlinkId, HyperlinkUri, is_allowed};
    pub use crate::interpreter::apc::ApcWebviewVerb;
    pub use crate::placement::{PlacementId, ProjectedPlacement};
    pub use crate::screen::cursor::{CURSOR_VISIBLE_BIT, Cursor, CursorShape};
    pub use crate::screen::grid::GridSize;
    pub use crate::screen::grid::coords::{GridColumn, GridLine, GridPoint, ScreenLine};
    pub use crate::screen::grid::row::Row;
    pub use crate::screen::grid::run::{Run, Style};
    pub use crate::screen::viewport::{DisplayOffset, Scroll, ViewportLine};
    pub use crate::selection::{CellSide, SelectionGeometry, SelectionKind, SelectionRange};
    pub use crate::vi::{ViCursor, ViModeSwitch};
    pub use crate::{OrzmaVt, Vt, VtSignal, VtUpdate};
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
/// [`crate::screen::grid::row::Row`] / [`crate::screen::grid::run::Run`] data, so the trait
/// exposes no per-cell read seam and the VT's storage cell never
/// leaves the crate.
pub trait Vt {
    /// Interprets one PTY chunk, staging its damage internally and
    /// returning everything else it produced.
    ///
    /// An empty chunk returns [`VtUpdate::default`].
    ///
    /// # Invariants
    ///
    /// - [`VtUpdate::signals`] preserves byte-stream order.
    /// - [`VtUpdate::replies`] must be written back to the PTY.
    ///
    /// # Webview placements
    ///
    /// An APC webview `mount` becomes a [`VtSignal::ApcWebview`] whose
    /// [`crate::placement::PlacementId`] the VT mints itself; `placement:
    /// None` is a policy rejection. The VT owns the placement table
    /// and projects every placement into
    /// [`crate::frame::Frame::placements`] on each emit; a mount,
    /// unmount, eviction, or projected-geometry change always raises
    /// the chunk liveness, so the frame carrying the new list is
    /// guaranteed to follow. Evictions the VT performs on its own authority
    /// (history trim, alternate-screen teardown) surface as
    /// [`VtSignal::WebviewEvicted`]. Ids are never reused within a
    /// session.
    ///
    /// A placement projects only while the screen it was mounted on is
    /// active: while the alternate screen is shown, primary-screen
    /// placements are omitted from the emitted lists (hidden, not
    /// evicted), and the reverse on returning to the primary screen. A
    /// re-issued `mount` for a live `(view_id, instance)` registers a
    /// successor under a fresh id; the superseded id simply stops
    /// being listed and is never named by
    /// [`VtSignal::WebviewEvicted`].
    fn interpret(&mut self, chunk: &[u8]) -> VtUpdate;

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

    /// Resizes the grid, reflowing content; returns whether the
    /// dimensions changed. Only a real change stages (full) damage.
    ///
    /// # Invariants
    ///
    /// Both axes are nonzero; degenerate sizes are rejected by the
    /// caller.
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
pub struct VtUpdate {
    /// Whether this chunk produced anything frame-relevant — staged row
    /// damage, cursor motion, or a mutated frame-visible section — so
    /// the owner knows to open its coalesce window.
    pub damaged: bool,
    /// Out-of-band signals, in byte-stream order.
    pub signals: Vec<VtSignal>,
    /// Reply bytes (DSR, DA, …) the owner must write back to the PTY.
    pub replies: Vec<u8>,
}

/// Out-of-band signal parsed from the VT byte stream, handed to the
/// owner in [`VtUpdate::signals`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VtSignal {
    /// An audible bell has been requested; the consumer is responsible
    /// for audio output or visual feedback (e.g. a flash).
    Bell,
    /// The application set an OS title string (OSC 0 or OSC 2). The owner
    /// typically uses this to set the window or tab title.
    Title(String),
    /// The application reset the OS title strings to their defaults (OSC 1
    /// or OSC 2 with an empty argument). The owner typically uses this to
    /// restore the window or tab title.
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
    ApcWebview {
        /// The mount or unmount verb and associated metadata.
        verb: ApcWebviewVerb,
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
    #[expect(
        dead_code,
        reason = "OrzmaVt::interpret reaches the parser once the executor's callbacks land"
    )]
    interpreter: Interpreter,
    /// The emulated device: screens, modes, tabs, colors, title.
    device: DeviceState,
    /// Webview placements: minting, anchor tracking, projection.
    placements: PlacementStore,
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
    /// The tracker must come from [`FrameTracker::new`]: its seeded
    /// full damage is what makes the first frame carry every viewport
    /// row, so a constructor that starts from an empty ledger paints
    /// nothing until the first PTY output arrives.
    pub fn new(size: GridSize, max_history: usize) -> Self {
        Self {
            interpreter: Interpreter::default(),
            device: DeviceState::new(size, max_history),
            placements: PlacementStore::new(),
            tracker: FrameTracker::new(),
        }
    }
}

impl Vt for OrzmaVt {
    // NOTE: The empty-chunk guard is load-bearing: the contract pins
    // "an empty chunk returns VtUpdate::default()", and parsing zero
    // bytes would still classify the damage a previous chunk left
    // staged.
    fn interpret(&mut self, chunk: &[u8]) -> VtUpdate {
        if chunk.is_empty() {
            return VtUpdate::default();
        }
        todo!()
    }

    fn frame(&mut self) -> Option<Frame> {
        self.tracker.emit(&self.device, &self.placements)
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
    use crate::screen::viewport::ViewportLine;

    fn vt() -> OrzmaVt {
        OrzmaVt::new(GridSize { cols: 4, rows: 3 }, 10)
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
        let damage = vt.device.active_mut().print('x');
        vt.tracker.stage_if_changed(damage);
        for _ in 0..3 {
            vt.device.active_mut().line_feed();
        }
        vt.device.active_mut().set_display_offset(DisplayOffset(1));
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
        vt.placements
            .mount(vt.device.active_screen(), 2, 4, "v".to_string(), None)
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
}
