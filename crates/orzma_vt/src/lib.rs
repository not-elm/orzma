//! Terminal emulation for orzma.
//!
//! [`schema`] declares the vocabulary; the crate root defines [`Vt`],
//! the protocol between a self-contained terminal emulator and its
//! owner. [`vt`] holds the superseded [`vt::OldOrzmaVt`] +
//! [`vt::VtBackend`] pair until the migration to [`Vt`] completes.

use crate::{
    damage::DamageVerdict,
    schema::{Frame, GridSize, Scroll, VtModes, VtSignal},
    screen::viewport::DisplayOffset,
};

pub mod damage;
mod frame;
pub mod hyperlink;
pub mod schema;
pub mod screen;
pub mod vt;

pub mod prelude {
    pub use crate::{Vt, VtUpdate, damage::*, schema::*, vt::*};
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
    /// Damage classification for the owner's flush decision; `None`
    /// when no damage cycle ran (an empty chunk).
    pub verdict: Option<DamageVerdict>,
    /// Out-of-band signals, in byte-stream order.
    pub signals: Vec<VtSignal>,
    /// Reply bytes (DSR, DA, …) the owner must write back to the PTY.
    pub replies: Vec<u8>,
}

/// The forthcoming self-contained implementation of [`Vt`], replacing
/// the [`vt::OldOrzmaVt`] + [`vt::VtBackend`] pair.
///
/// Every method is still a stub; the grid, damage tracking, and frame
/// builder land with the migration tracked in
/// `docs/orzma_tty_engine_replacement_gaps.md`.
pub struct OrzmaVt {}

impl Vt for OrzmaVt {
    fn interpret(&mut self, _chunk: &[u8]) -> VtUpdate {
        todo!()
    }

    fn frame(&mut self) -> Option<Frame> {
        todo!()
    }

    fn resize(&mut self, _size: GridSize) -> bool {
        todo!()
    }

    fn scroll(&mut self, _scroll: Scroll) -> bool {
        todo!()
    }

    fn grid_size(&self) -> GridSize {
        todo!()
    }

    fn display_offset(&self) -> DisplayOffset {
        todo!()
    }

    fn modes(&self) -> VtModes {
        todo!()
    }
}
