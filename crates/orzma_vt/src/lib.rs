//! Terminal emulation for orzma.
//!
//! [`schema`] declares the vocabulary; the crate root defines [`Vt`],
//! the protocol between a self-contained terminal emulator and its
//! owner, and [`OrzmaVt`], the implementation of that protocol.

use crate::{
    damage::DamageLedger,
    device::DeviceState,
    interpreter::Interpreter,
    placement::PlacementStore,
    schema::{Frame, GridSize, Scroll, VtModes, VtSignal},
    screen::viewport::DisplayOffset,
};

pub mod damage;
mod device;
pub mod frame;
pub mod hyperlink;
mod interpreter;
mod placement;
pub mod schema;
pub mod screen;

pub mod prelude {
    pub use crate::{OrzmaVt, Vt, VtUpdate, damage::*, schema::*};
}

/// The terminal-emulation contract `OrzmaTerm` drives and the host
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
/// [`crate::schema::Row`] / [`crate::schema::Run`] data, so the trait
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
    /// [`crate::schema::PlacementId`] the VT mints itself; `placement:
    /// None` is a policy rejection. The VT owns the placement table
    /// and projects every placement into
    /// [`crate::schema::FrameSnapshot::placements`] /
    /// [`crate::schema::FrameDelta::placements`] on each emit; a
    /// mount, unmount, eviction, or projected-geometry change always
    /// stages damage, so the frame carrying the new list is guaranteed
    /// to follow. Evictions the VT performs on its own authority
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

    /// Builds the frame for the staged damage, consuming it; `None`
    /// when nothing is staged.
    ///
    /// Staged full damage yields a [`Frame::Snapshot`]; row damage
    /// yields a [`Frame::Delta`], possibly empty with current metadata.
    ///
    /// # Invariants
    ///
    /// - The first emitted frame, and every alternate-screen flip, is a
    ///   [`Frame::Snapshot`].
    /// - A frame's placements and display offset describe the
    ///   same instant as its rows.
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
    /// Whether this chunk staged any damage, so the owner knows to open
    /// its coalesce window. Metadata-only damage counts: the frame that
    /// carries the new placement list must still be emitted.
    pub damaged: bool,
    /// Out-of-band signals, in byte-stream order.
    pub signals: Vec<VtSignal>,
    /// Reply bytes (DSR, DA, …) the owner must write back to the PTY.
    pub replies: Vec<u8>,
}

/// The self-contained implementation of [`Vt`].
///
/// The fields are wired; several methods are still stubs. The
/// components land one at a time, in the order
/// `docs/orzma_vt_internal_design.md` §7 sets out.
#[expect(
    dead_code,
    reason = "the Vt methods read these fields once their components land"
)]
pub struct OrzmaVt {
    /// Byte decoding plus the CSI ?2026 synchronized-update buffer.
    interpreter: Interpreter,
    /// The emulated device: screens, modes, tabs, colors, title.
    device: DeviceState,
    /// Webview placements: minting, anchor tracking, projection.
    placements: PlacementStore,
    /// Damage staged for the next emit, from every source.
    damage: DamageLedger,
}

impl OrzmaVt {
    /// Builds a terminal whose first frame is a full snapshot.
    ///
    /// # Invariants
    ///
    /// Both grid axes are nonzero; degenerate sizes are rejected by the
    /// caller (the same contract as [`Vt::resize`]).
    ///
    /// The ledger must come from [`DamageLedger::new`]: its seeded full
    /// damage is what makes that first frame a snapshot, so a
    /// constructor that starts from an empty ledger paints nothing
    /// until the first PTY output arrives.
    pub fn new(size: GridSize, max_history: usize) -> Self {
        Self {
            interpreter: Interpreter::default(),
            device: DeviceState::new(size, max_history),
            placements: PlacementStore::new(),
            damage: DamageLedger::new(),
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
        Some(Frame::emit(
            self.damage.take()?,
            &self.device,
            &self.placements,
        ))
    }

    fn resize(&mut self, size: GridSize) -> bool {
        self.damage.stage_if_changed(self.device.resize(size))
    }

    fn scroll(&mut self, scroll: Scroll) -> bool {
        self.damage.stage_if_changed(self.device.scroll(scroll))
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
    use crate::schema::ViewportLine;

    fn vt() -> OrzmaVt {
        OrzmaVt::new(GridSize { cols: 4, rows: 3 }, 10)
    }

    /// Asserts that a fresh terminal's first frame is a full snapshot.
    ///
    /// The agreed mechanism is the ledger's seeded full damage rather
    /// than a first-emit flag on the VT: a flag would have to be cleared
    /// in every emit path, while the seed is spent by the same `take`
    /// every other frame goes through.
    ///
    /// Case: a terminal spawns and the renderer has nothing on screen
    /// yet, so the shell's first prompt must arrive with the whole
    /// viewport behind it.
    #[test]
    fn the_first_frame_is_a_snapshot() {
        assert!(matches!(vt().frame(), Some(Frame::Snapshot(_))));
    }

    /// Asserts that emitting drains the staged damage.
    ///
    /// Case: the host polls for a frame twice in one tick, and the
    /// second poll must not repaint what the first one already sent.
    #[test]
    fn an_emitted_frame_leaves_nothing_staged() {
        let mut vt = vt();
        vt.frame();
        assert!(vt.frame().is_none());
    }

    /// Asserts that a print's viewport-space damage survives the
    /// `Screen` → `DamageLedger` → `Frame` seam without the row it names
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
        vt.damage.stage_if_changed(damage);
        for _ in 0..3 {
            vt.device.active_mut().line_feed();
        }
        vt.device.active_mut().set_display_offset(DisplayOffset(1));
        let Some(Frame::Delta(delta)) = vt.frame() else {
            panic!("staged row damage emits a delta");
        };
        assert_eq!(delta.dirty_rows[0].line, ViewportLine(0));
        assert_eq!(delta.dirty_rows[0].contents[0].text, "x   ");
    }
}
