//! Terminal emulation for orzma.
//!
//! [`schema`] declares the vocabulary; the crate root defines [`Vt`],
//! the protocol between a self-contained terminal emulator and its
//! owner. [`vt`] holds the superseded [`vt::OldOrzmaVt`] +
//! [`vt::VtBackend`] pair until the migration to [`Vt`] completes.

use crate::schema::{
    DamageVerdict, DisplayOffset, Frame, GridCell, GridPoint, GridSize, Scroll, VtModes, VtSignal,
};

pub mod hyperlink;
pub mod schema;
pub mod vt;

pub mod prelude {
    pub use crate::{Vt, VtUpdate, schema::*, vt::*};
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
    /// # Webview anchors
    ///
    /// An APC webview `mount` becomes a [`VtSignal::ApcWebview`] whose
    /// anchor the VT stamps itself:
    ///
    /// - The cursor is sampled at the APC's byte position, including
    ///   bytes buffered by `CSI ?2026`.
    /// - Primary screen: [`crate::schema::AnchorMode::Scrollback`] with
    ///   `line = history_base + history_size + cursor_row`; alternate
    ///   screen: [`crate::schema::AnchorMode::FixedScreen`].
    /// - `frame_seq` is the next emitted seq, and a mount always stages
    ///   damage, so that frame is guaranteed to follow.
    /// - Only `Mount` carries `anchor: Some(..)`.
    /// - `history_base` grows monotonically: a scrollback clear folds
    ///   the shrink into it and synthesizes an unmount-all BEFORE
    ///   same-chunk anchors; a resize reflow re-baselines without
    ///   folding.
    /// - At the scrollback cap the VT synthesizes one unmount-all and
    ///   swallows primary-screen mounts; alternate-screen mounts are
    ///   exempt.
    fn interpret(&mut self, chunk: &[u8]) -> VtUpdate;

    /// Builds the frame for the staged damage, consuming it; `None`
    /// when nothing is staged.
    ///
    /// Staged full damage yields a [`Frame::Snapshot`]; row damage
    /// yields a [`Frame::Delta`], possibly empty with current metadata.
    ///
    /// # Invariants
    ///
    /// - The seq advances by one (wrapping) per emitted frame, never on
    ///   `None`; consumers compare it wrap-aware (distance < `2^31`).
    /// - The first emitted frame, and every alternate-screen flip, is a
    ///   [`Frame::Snapshot`].
    /// - A frame's history counters and display offset describe the
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

    /// Reads the cell at `point`; `None` when out of range — never a
    /// panic. Negative lines reach scrollback history.
    ///
    /// No caller exists yet; this is the read seam for cell-level host
    /// features such as hyperlink hover.
    fn cell_at(&self, point: GridPoint) -> Option<GridCell>;
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
