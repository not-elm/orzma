use crate::{
    damage::DamageVerdict, frame::Frame, modes::VtModes, prelude::VtSignal, scroll::Scroll,
};

#[cfg(feature = "alacritty")]
mod alacritty;

#[cfg(feature = "alacritty")]
pub use alacritty::AlacrittyVt;

pub trait OrzmaVt: Sized {
    fn new(cols: u16, rows: u16) -> Self;

    /// Number of rows the viewport sits above the live tail.
    ///
    /// `0` means the viewport is pinned to the live tail; a positive
    /// value counts the scrollback rows showing above it. The unit is
    /// grid rows, and the value never exceeds the backend's scrollback
    /// capacity.
    ///
    /// # Invariants
    ///
    /// The alternate screen carries no scrollback, so this stays `0`
    /// for as long as it is active.
    fn display_offset(&self) -> u32;

    /// Returns `true` when the viewport is pinned to the live tail.
    #[inline]
    fn at_scroll_bottom(&self) -> bool {
        self.display_offset() == 0
    }

    /// Interprets a chunk of the PTY byte stream, mutating the terminal
    /// state, and classifies the resulting damage. ("Interpret" per
    /// ECMA-48 § 2.3.3: a receiving device interprets the coded
    /// representations of control functions.)
    fn interpret(&mut self, chunk: &[u8]) -> Option<DamageVerdict>;

    /// Builds the frame for the staged damage.
    fn frames(&mut self) -> Vec<Frame>;

    fn drain_signals(&mut self) -> impl Iterator<Item = VtSignal> + '_;

    /// DSR/DA reply bytes the owner must write back to the PTY.
    fn drain_replies_into(&self, buf: &mut Vec<u8>);

    /// Applies the given viewport motion.
    ///
    /// Every motion clamps to the scrollback bounds, so a call that
    /// lands where the viewport already is leaves it untouched
    /// (idempotent at the boundary — [`Scroll::Bottom`] at the live
    /// tail is a no-op).
    ///
    /// # Invariants
    ///
    /// A no-op call stages no damage, so a caller that needs the
    /// renderer to observe the new viewport must gate on
    /// [`Self::at_scroll_bottom`] rather than scrolling
    /// unconditionally.
    ///
    /// # References
    ///
    /// - [XTerm Control Sequences] — DECSET 1011 (`scrollKey`): scroll
    ///   to bottom on key press; [`Scroll::Bottom`] is the mechanism
    ///   behind that scroll-on-input policy. The inverse policy, DECSET
    ///   1010 (`scrollTtyOutput`), is not implemented by this trait:
    ///   the viewport holds its position while the PTY emits output.
    ///
    /// [XTerm Control Sequences]: https://invisible-island.net/xterm/ctlseqs/ctlseqs.html
    fn scroll(&mut self, scroll: Scroll);

    /// Snapshot of the input-relevant terminal modes.
    fn modes(&self) -> VtModes;

    /// Resizes the emulated grid to `cols` x `rows` cells.
    ///
    /// Stages full damage so the next [`Self::frames`] call repaints
    /// the reflowed grid.
    ///
    /// # Invariants
    ///
    /// Both dimensions must be nonzero: degenerate-size validation is
    /// the caller's job (`OrzmaTerm::resize` ignores zero-axis and
    /// oversized requests before this method is reached).
    fn resize(&mut self, cols: u16, rows: u16);

    /// Grid dimensions as `(cols, rows)`.
    ///
    /// The row count is the source of truth for "one screenful"
    /// (scroll paging) and for verifying an applied resize.
    fn grid_size(&self) -> (u16, u16);
}
