//! Engine layer: the [`OrzmaVt`] contract and its backends.

use crate::schema::{
    Damage, DisplayOffset, Frame, GridSize, Scroll, SelectionKind, SelectionOp, SelectionRange,
    ViModeSwitch, VtModes, VtResult, VtSignal,
};

mod apc;

#[cfg(feature = "alacritty")]
mod alacritty;

#[cfg(feature = "alacritty")]
pub use alacritty::AlacrittyVtBackend;

pub struct OrzmaVt<B: VtBackend> {
    backend: B,
    pending_damage: Option<Damage>,
}

impl<B: VtBackend> OrzmaVt<B> {
    /// Constructs the new orzma vt.
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            backend: B::new(cols, rows),
            pending_damage: None,
        }
    }

    pub fn interpret(&mut self, chunk: &[u8]) {}
}

pub trait VtBackend: Sized {
    fn new(cols: u16, rows: u16) -> Self;

    /// Number of scrollback rows the viewport sits above the live tail.
    fn display_offset(&self) -> DisplayOffset;

    /// Returns `true` when the viewport is pinned to the live tail.
    #[inline]
    fn is_at_live_tail(&self) -> bool {
        self.display_offset() == DisplayOffset(0)
    }

    /// Interprets a chunk of the PTY byte stream, mutating the terminal
    /// state, and returns the collected damage. ("Interpret" per
    /// ECMA-48 § 2.3.3: a receiving device interprets the coded
    /// representations of control functions.)
    ///
    /// `None` for an empty chunk — not a damage cycle. Classifying the
    /// damage is the caller's job.
    fn interpret(&mut self, chunk: &[u8]) -> Option<Damage>;

    fn drain_signals(&mut self) -> impl Iterator<Item = VtSignal> + '_;

    /// DSR/DA reply bytes the owner must write back to the PTY.
    fn drain_replies_into(&self, buf: &mut Vec<u8>);

    /// Applies the given viewport motion.
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

    /// Grid dimensions in cells.
    fn grid_size(&self) -> GridSize;

    /// Applies one selection operation.
    ///
    /// An operation that changes the visible selection stages full
    /// damage so the next [`Self::frames`] call repaints — the backing
    /// emulator's damage tracking does not cover selection state, so
    /// implementations must stage it themselves. An operation that
    /// changes nothing (an `UpdateTo` with no active selection, a
    /// redundant `Clear`) stages no damage.
    fn apply_selection(&mut self, op: SelectionOp) -> VtResult;

    /// The active selection as normalized viewport coordinates.
    ///
    /// `None` when no selection exists or the active one is empty.
    /// Callers compare this before and after [`Self::apply_selection`]
    /// to detect a real change — the same role [`Self::display_offset`]
    /// plays for [`Self::scroll`].
    fn selection_range(&self) -> Option<SelectionRange>;

    /// The active selection's granularity.
    ///
    /// `None` when no selection exists. The vi `v` / `V` handling reads
    /// this to decide between clearing (same kind), switching kind, and
    /// starting a new selection.
    fn selection_kind(&self) -> Option<SelectionKind>;

    /// The selected text, honoring wrapped lines, wide characters, and
    /// the Block / Lines shapes.
    ///
    /// `None` when no selection exists or the active one is empty.
    fn selected_text(&self) -> Option<String>;

    /// Switches the vi-mode of the terminal to [`ViModeSwitch`].
    fn switch_vi_mode(&mut self, vi_mode: ViModeSwitch) -> VtResult;
}
