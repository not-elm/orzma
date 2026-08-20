//! Terminal emulation for orzma.
//!
//! [`schema`] declares the vocabulary; the crate root defines [`Vt`],
//! the protocol between a self-contained terminal emulator and its
//! owner. [`vt`] holds the superseded [`vt::OldOrzmaVt`] +
//! [`vt::VtBackend`] pair until the migration to [`Vt`] completes.

use crate::{
    damage::{DamageLedger, DamageVerdict},
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
/// The fields are wired; every method is still a stub. The components
/// land one at a time, in the order
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
    /// The ledger must come from [`DamageLedger::new`]: its seeded full
    /// damage is what makes that first frame a snapshot, so a
    /// constructor that starts from an empty ledger paints nothing
    /// until the first PTY output arrives.
    pub fn new(_size: GridSize, _max_history: usize) -> Self {
        todo!()
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

    // TODO: Land with the delta path: `Damage::Full` already has a
    // builder in `frame.rs`, but staged row damage has no `FrameDelta`
    // to become, and returning a snapshot for it would break the
    // contract above.
    fn frame(&mut self) -> Option<Frame> {
        todo!()
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
