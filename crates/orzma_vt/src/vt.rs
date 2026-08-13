//! Engine layer: the [`OrzmaVt`] contract and its backends.

use crate::schema::{
    Damage, DamageRows, DamageVerdict, DisplayOffset, Frame, GridSize, Scroll, SelectionKind,
    SelectionOp, SelectionRange, ViModeSwitch, VtModes, VtResult, VtSignal,
};

mod apc;

#[cfg(feature = "alacritty")]
mod alacritty;

#[cfg(all(test, feature = "alacritty"))]
mod tests;

#[cfg(feature = "alacritty")]
pub use alacritty::AlacrittyVtBackend;

/// A [`VtBackend`] plus the damage staged for the next frame emit.
///
/// The backend reports the damage each mutation produced; this wrapper
/// owns accumulating it across calls until an emit consumes it.
pub struct OrzmaVt<B: VtBackend> {
    backend: B,
    /// Damage staged for the next frame emit.
    ///
    /// Merged rather than replaced on each stage: the backend reports
    /// per-call damage, so an overwritten staged value would lose a
    /// repaint no later call re-reports.
    pending_damage: Option<Damage>,
}

impl<B: VtBackend> OrzmaVt<B> {
    /// Builds the backend and stages the bootstrap repaint.
    ///
    /// The staged `Full` guarantees the first emit paints the whole
    /// grid even when the shell never writes a byte.
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            backend: B::new(cols, rows),
            pending_damage: Some(Damage::Full),
        }
    }

    /// Interprets a PTY chunk, stages the damage it produced, and
    /// classifies that damage for the caller's flush decision.
    ///
    /// `None` for an empty chunk — not a damage cycle.
    pub fn interpret(&mut self, chunk: &[u8]) -> Option<DamageVerdict> {
        let damage = self.backend.interpret(chunk)?;
        let verdict = DamageVerdict::classify(&damage);
        self.stage(damage);
        Some(verdict)
    }

    /// Applies the viewport motion; returns whether the viewport moved.
    pub fn scroll(&mut self, scroll: Scroll) -> bool {
        let damage = self.backend.scroll(scroll);
        self.stage_if_changed(damage)
    }

    /// Resizes the grid; returns whether the dimensions changed.
    pub fn resize(&mut self, cols: u16, rows: u16) -> bool {
        let damage = self.backend.resize(cols, rows);
        self.stage_if_changed(damage)
    }

    /// Applies one selection operation; returns whether the visible
    /// selection changed.
    pub fn apply_selection(&mut self, op: SelectionOp) -> VtResult<bool> {
        let damage = self.backend.apply_selection(op)?;
        Ok(self.stage_if_changed(damage))
    }

    /// Switches vi mode; returns whether the mode actually flipped.
    pub fn switch_vi_mode(&mut self, vi_mode: ViModeSwitch) -> VtResult<bool> {
        let damage = self.backend.switch_vi_mode(vi_mode)?;
        Ok(self.stage_if_changed(damage))
    }

    /// Number of scrollback rows the viewport sits above the live tail.
    #[inline]
    pub fn display_offset(&self) -> DisplayOffset {
        self.backend.display_offset()
    }

    /// Returns `true` when the viewport is pinned to the live tail.
    #[inline]
    pub fn is_at_live_tail(&self) -> bool {
        self.backend.is_at_live_tail()
    }

    /// Grid dimensions in cells.
    #[inline]
    pub fn grid_size(&self) -> GridSize {
        self.backend.grid_size()
    }

    /// Snapshot of the input-relevant terminal modes.
    #[inline]
    pub fn modes(&self) -> VtModes {
        self.backend.modes()
    }

    /// The active selection as normalized viewport coordinates.
    #[inline]
    pub fn selection_range(&self) -> Option<SelectionRange> {
        self.backend.selection_range()
    }

    /// The active selection's granularity.
    #[inline]
    pub fn selection_kind(&self) -> Option<SelectionKind> {
        self.backend.selection_kind()
    }

    /// The selected text.
    #[inline]
    pub fn selected_text(&self) -> Option<String> {
        self.backend.selected_text()
    }

    /// Out-of-band signals drained from the backend.
    #[inline]
    pub fn drain_signals(&mut self) -> impl Iterator<Item = VtSignal> + '_ {
        self.backend.drain_signals()
    }

    /// DSR/DA reply bytes the owner must write back to the PTY.
    #[inline]
    pub fn drain_replies_into(&self, buf: &mut Vec<u8>) {
        self.backend.drain_replies_into(buf)
    }

    /// Merges `damage` into the staged value.
    ///
    /// Seeding an absent staged value with an empty row set is safe
    /// because that set is the merge identity.
    fn stage(&mut self, damage: Damage) {
        *self
            .pending_damage
            .get_or_insert(Damage::Delta(DamageRows::default())) |= damage;
    }

    /// Stages the reported damage, if any; returns whether there was
    /// any to stage.
    fn stage_if_changed(&mut self, damage: Option<Damage>) -> bool {
        match damage {
            Some(damage) => {
                self.stage(damage);
                true
            }
            None => false,
        }
    }
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
    /// state, and returns the damage THIS chunk produced — the backend
    /// reads and resets its tracker every call, so consecutive returns
    /// never overlap. ("Interpret" per ECMA-48 § 2.3.3: a receiving
    /// device interprets the coded representations of control
    /// functions.)
    ///
    /// `None` for an empty chunk — not a damage cycle. Classifying and
    /// accumulating the damage is the caller's job.
    fn interpret(&mut self, chunk: &[u8]) -> Option<Damage>;

    fn drain_signals(&mut self) -> impl Iterator<Item = VtSignal> + '_;

    /// DSR/DA reply bytes the owner must write back to the PTY.
    fn drain_replies_into(&self, buf: &mut Vec<u8>);

    /// Applies the given viewport motion.
    ///
    /// Returns `Damage::Full` when the viewport actually moved; `None`
    /// for a clamped or zero motion.
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
    fn scroll(&mut self, scroll: Scroll) -> Option<Damage>;

    /// Snapshot of the input-relevant terminal modes.
    fn modes(&self) -> VtModes;

    /// Resizes the emulated grid to `cols` x `rows` cells.
    ///
    /// Returns `Damage::Full` when the dimensions changed; `None` when
    /// they already matched.
    ///
    /// # Invariants
    ///
    /// Both dimensions must be nonzero: degenerate-size validation is
    /// the caller's job (`OrzmaTerm::resize` ignores zero-axis and
    /// oversized requests before this method is reached).
    fn resize(&mut self, cols: u16, rows: u16) -> Option<Damage>;

    /// Grid dimensions in cells.
    fn grid_size(&self) -> GridSize;

    /// Applies one selection operation.
    ///
    /// `Ok(Some(_))` reports the repaint for a visible selection change
    /// — the backing emulator's damage tracking does not cover
    /// selection state, so the caller can learn of it only here.
    /// Conservative over-reporting is allowed. `Ok(None)` = a genuine
    /// no-op (an `UpdateTo`, `ChangeKind`, or `Clear` with no active
    /// selection).
    fn apply_selection(&mut self, op: SelectionOp) -> VtResult<Option<Damage>>;

    /// The active selection as normalized viewport coordinates.
    ///
    /// `None` when no selection exists or the active one is empty.
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
    ///
    /// `Ok(Some(Damage::Full))` on a real transition — the vi cursor
    /// overlay appears or disappears outside the backing emulator's
    /// damage tracking. `Ok(None)` on an idempotent request.
    fn switch_vi_mode(&mut self, vi_mode: ViModeSwitch) -> VtResult<Option<Damage>>;
}
