//! Engine layer: the [`OrzmaVt`] contract and its backends.

use crate::schema::{
    CellSide, Damage, DamageRows, DamageVerdict, DisplayOffset, Frame, GridSize, Scroll,
    SelectionKind, SelectionRange, ViModeSwitch, ViewportPoint, VtModes, VtResult, VtSignal,
};

#[cfg(feature = "alacritty")]
mod alacritty;
mod apc;

#[cfg(feature = "alacritty")]
pub use alacritty::AlacrittyVtBackend;

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
    /// Constructs the new vt.
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            backend: B::new(cols, rows),
            pending_damage: Some(Damage::Full),
        }
    }

    /// Interprets a PTY chunk
    pub fn interpret(&mut self, chunk: &[u8]) -> Option<DamageVerdict> {
        let damage = self.backend.interpret(chunk)?;
        let verdict = DamageVerdict::classify(&damage);
        self.stage(damage);
        Some(verdict)
    }

    pub fn frame(&mut self) -> Option<Frame> {
        let damage = self.pending_damage.take()?;
        todo!()
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

impl<B: VtBackend + VtSelection> OrzmaVt<B> {
    /// Anchors a new selection at an explicit viewport cell; returns
    /// whether the backend reported a repaint to stage.
    ///
    /// [`VtSelection`] allows conservative over-reporting, so this is
    /// not a "the visible selection changed" signal: re-anchoring onto
    /// the already-selected cell still reports a repaint. A caller that
    /// needs the precise gate compares
    /// [`Self::selection_range`] before and after instead.
    pub fn start_selection(
        &mut self,
        cell: ViewportPoint,
        side: CellSide,
        kind: SelectionKind,
    ) -> VtResult<bool> {
        let damage = self.backend.start_selection(cell, side, kind)?;
        Ok(self.stage_if_changed(damage))
    }

    /// Anchors a new selection at the vi cursor; returns whether the
    /// backend reported a repaint to stage.
    pub fn start_selection_at_vi_cursor(&mut self, kind: SelectionKind) -> VtResult<bool> {
        let damage = self.backend.start_selection_at_vi_cursor(kind)?;
        Ok(self.stage_if_changed(damage))
    }

    /// Moves the moving end of the active selection; returns whether
    /// the backend reported a repaint to stage. A drag sample that
    /// lands back on the cell and side it came from still reports one.
    pub fn update_selection(&mut self, cell: ViewportPoint, side: CellSide) -> VtResult<bool> {
        let damage = self.backend.update_selection(cell, side)?;
        Ok(self.stage_if_changed(damage))
    }

    /// Switches selection granularity while keeping the anchor;
    /// returns whether the backend reported a repaint to stage.
    pub fn change_selection_kind(&mut self, kind: SelectionKind) -> VtResult<bool> {
        let damage = self.backend.change_selection_kind(kind)?;
        Ok(self.stage_if_changed(damage))
    }

    /// Drops any active selection; returns whether one was dropped.
    pub fn clear_selection(&mut self) -> VtResult<bool> {
        let damage = self.backend.clear_selection()?;
        Ok(self.stage_if_changed(damage))
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

    /// Switches the vi-mode of the terminal to [`ViModeSwitch`].
    ///
    /// `Ok(Some(Damage::Full))` on a real transition — the vi cursor
    /// overlay appears or disappears outside the backing emulator's
    /// damage tracking. `Ok(None)` on an idempotent request.
    fn switch_vi_mode(&mut self, vi_mode: ViModeSwitch) -> VtResult<Option<Damage>>;
}

/// Selection capability of a VT backend.
///
/// Split from [`VtBackend`] so a backend without selection support
/// carries no selection API, and so [`OrzmaVt`] exposes its selection
/// surface only for backends that implement this trait.
///
/// Every mutator returns the repaint it produced: the backing
/// emulator's damage tracking does not cover selection state, so the
/// caller can learn of a visible selection change only here.
/// Conservative over-reporting is allowed; `Ok(None)` is a genuine
/// no-op.
pub trait VtSelection {
    /// Anchors a new selection at an explicit viewport cell (mouse
    /// press).
    fn start_selection(
        &mut self,
        cell: ViewportPoint,
        side: CellSide,
        kind: SelectionKind,
    ) -> VtResult<Option<Damage>>;

    /// Anchors a new selection at the vi cursor (vi-mode `v` / `V`),
    /// whose position only the VT knows.
    fn start_selection_at_vi_cursor(&mut self, kind: SelectionKind) -> VtResult<Option<Damage>>;

    /// Moves the moving end of the active selection to a viewport cell
    /// (mouse drag). The cell may sit outside the viewport when the
    /// drag leaves it. `Ok(None)` when nothing is selected.
    fn update_selection(&mut self, cell: ViewportPoint, side: CellSide)
    -> VtResult<Option<Damage>>;

    /// Switches granularity while keeping the anchor (vi-mode `v`
    /// while `V` is active, and the reverse). `Ok(None)` when nothing
    /// is selected.
    fn change_selection_kind(&mut self, kind: SelectionKind) -> VtResult<Option<Damage>>;

    /// Drops any active selection. `Ok(None)` when nothing was
    /// selected.
    fn clear_selection(&mut self) -> VtResult<Option<Damage>>;

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
}

#[cfg(all(test, feature = "alacritty"))]
mod tests {
    use super::*;

    /// Builds a wrapper whose bootstrap damage and backend accumulator are
    /// both consumed, so a test observes only what its own calls stage.
    fn clean_vt() -> OrzmaVt<AlacrittyVtBackend> {
        let mut vt = OrzmaVt::new(80, 24);
        vt.interpret(b"\x1b[H");
        vt.pending_damage = None;
        vt
    }

    fn vt_with_history() -> OrzmaVt<AlacrittyVtBackend> {
        let mut vt = OrzmaVt::new(80, 24);
        let seed: Vec<u8> = (0..40)
            .flat_map(|i| format!("l{i}\r\n").into_bytes())
            .collect();
        vt.interpret(&seed);
        vt.pending_damage = None;
        vt
    }

    fn start_simple(vt: &mut OrzmaVt<AlacrittyVtBackend>, x: u16, y: i16) -> bool {
        vt.start_selection(
            ViewportPoint { row: y, column: x },
            CellSide::Left,
            SelectionKind::Simple,
        )
        .unwrap()
    }

    /// Asserts that a fresh wrapper stages the bootstrap full repaint.
    ///
    /// Case: the first emit precedes any PTY output from a shell that
    /// stays silent.
    #[test]
    fn a_fresh_vt_stages_bootstrap_full_damage() {
        let vt = OrzmaVt::<AlacrittyVtBackend>::new(80, 24);
        assert_eq!(vt.pending_damage, Some(Damage::Full));
    }

    /// Asserts that `interpret` stages the damage it classified.
    ///
    /// Case: ordinary shell output arrives between two emits.
    #[test]
    fn interpret_stages_the_damage_it_classified() {
        let mut vt = clean_vt();
        assert_eq!(
            vt.interpret(b"one\r\ntwo\r\nthree"),
            Some(DamageVerdict::ManyRows { rows: 3 })
        );
        assert_eq!(vt.pending_damage, Some(Damage::Delta(vec![0, 1, 2].into())));
    }

    /// Asserts that damage staged by an earlier chunk survives into the
    /// staged value of a later one.
    ///
    /// Case: two PTY chunks arrive between one emit and the next.
    #[test]
    fn staged_damage_accumulates_across_chunks() {
        let mut vt = clean_vt();
        vt.interpret(b"a");
        vt.interpret(b"\r\n\r\nb");
        let Some(Damage::Delta(rows)) = &vt.pending_damage else {
            panic!(
                "expected staged partial damage, got {:?}",
                vt.pending_damage
            );
        };
        assert!(rows.contains(&0), "row from the first chunk, got {rows:?}");
        assert!(rows.contains(&2), "row from the second chunk, got {rows:?}");
    }

    /// Asserts that an empty chunk neither reports a cycle nor disturbs
    /// the staged value.
    ///
    /// Case: a zero-length PTY read lands between two real chunks.
    #[test]
    fn an_empty_chunk_leaves_staged_damage_untouched() {
        let mut vt = clean_vt();
        vt.interpret(b"hi");
        let staged = vt.pending_damage.clone();
        assert!(
            staged.is_some(),
            "precondition: a real chunk must stage damage"
        );
        assert_eq!(vt.interpret(b""), None);
        assert_eq!(vt.pending_damage, staged);
    }

    /// Asserts that a viewport-moving scroll stages full damage and
    /// reports the move.
    ///
    /// Case: the user wheels into scrollback on an otherwise idle
    /// terminal.
    #[test]
    fn a_moving_scroll_stages_full_damage() {
        let mut vt = vt_with_history();
        assert!(vt.scroll(Scroll::Delta(3)));
        assert_eq!(vt.pending_damage, Some(Damage::Full));
    }

    /// Asserts that a selection change stages full damage and reports the
    /// change.
    ///
    /// Case: a mouse press anchors a selection on an otherwise idle
    /// terminal.
    #[test]
    fn a_selection_change_stages_full_damage() {
        let mut vt = clean_vt();
        assert!(start_simple(&mut vt, 0, 0));
        assert_eq!(vt.pending_damage, Some(Damage::Full));
    }

    /// Asserts that no-op operations preserve the staged value exactly.
    ///
    /// Case: clamped scrolls, stray selection ops after an alt-screen
    /// wipe, an idempotent vi request, and a same-size resize all arrive
    /// while damage from earlier output is still awaiting its emit.
    #[test]
    fn no_op_operations_preserve_staged_damage() {
        let mut vt = clean_vt();
        vt.pending_damage = Some(Damage::Delta(vec![0].into()));
        let staged = vt.pending_damage.clone();
        assert!(!vt.scroll(Scroll::Delta(0)));
        assert!(
            !vt.update_selection(ViewportPoint { row: 0, column: 2 }, CellSide::Right)
                .unwrap()
        );
        assert!(!vt.clear_selection().unwrap());
        assert!(!vt.switch_vi_mode(ViModeSwitch::Exit).unwrap());
        assert!(!vt.resize(80, 24));
        assert_eq!(vt.pending_damage, staged);
    }
}
