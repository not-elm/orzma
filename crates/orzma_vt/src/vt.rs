//! Engine layer: the [`OrzmaVt`] contract and its backends.

use crate::schema::{
    CellSide, Cursor, Damage, DamageRows, DamageVerdict, DisplayOffset, Frame, FrameSnapshot,
    GridPoint, GridSize, Hyperlink, Palette, Row, Scroll, SelectionKind, SelectionRange, ViCursor,
    ViModeSwitch, ViewportLine, VtModes, VtResult, VtSignal,
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
    /// Wrapping emission sequence stamped on the next frame; advances
    /// only when a frame is actually emitted.
    next_frame_seq: u32,
}

impl<B: VtBackend> OrzmaVt<B> {
    /// Constructs the new vt.
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            backend: B::new(cols, rows),
            pending_damage: Some(Damage::Full),
            next_frame_seq: 0,
        }
    }

    /// Interprets a PTY chunk
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
    /// Builds the frame for the staged damage, consuming it.
    ///
    /// Returns `None` when nothing is staged. Staged
    /// [`Damage::Full`] yields a [`Frame::Snapshot`]; staged row
    /// damage yields a [`Frame::Delta`] — including an empty one,
    /// whose metadata is still current.
    pub fn frame(&mut self) -> Option<Frame> {
        let damage = self.pending_damage.take()?;
        Some(Frame::Snapshot(FrameSnapshot {
            seq: 0,
            size: self.grid_size(),
            rows: todo!(),
            cursor: self.backend.cursor(),
            vi_cursor: self.backend.vi_cursor(),
            display_offset: self.backend.display_offset(),
            history_size: todo!(),
            history_base: todo!(),
            selection: self.backend.selection_range(),
            hyperlinks: todo!(),
            palette: todo!(),
        }))
    }

    /// Anchors a new selection at an explicit grid cell; returns
    /// whether the backend reported a repaint to stage.
    ///
    /// [`VtSelection`] allows conservative over-reporting, so this is
    /// not a "the visible selection changed" signal: re-anchoring onto
    /// the already-selected cell still reports a repaint. A caller that
    /// needs the precise gate compares
    /// [`Self::selection_range`] before and after instead.
    pub fn start_selection(
        &mut self,
        cell: GridPoint,
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
    pub fn update_selection(&mut self, cell: GridPoint, side: CellSide) -> VtResult<bool> {
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

    /// Returns the cursor info.
    fn cursor(&self) -> Cursor;

    /// Returns the cursor info.([`None`] if not in vi-mode)
    fn vi_cursor(&self) -> Option<ViCursor>;

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

    /// Extracts the contents of the given viewport rows — or of the
    /// whole visible viewport when `lines` is `None` — together with
    /// the hyperlinks those rows reference.
    ///
    /// One combined pass because hyperlink-id assignment is stateful:
    /// the backend owns the interner, and only the extraction knows
    /// which links the produced runs actually reference.
    fn extract_rows(&mut self, lines: Option<&DamageRows>) -> ExtractedRows;

    /// Total scrollback history line count.
    fn history_size(&self) -> u32;

    /// The live palette symbolic colors resolve against.
    fn palette(&self) -> Palette;
}

/// Rows extracted from the backend in one pass, together with the
/// hyperlinks those rows reference.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtractedRows {
    /// The extracted rows, ascending by viewport line.
    pub rows: Vec<(ViewportLine, Row)>,
    /// Hyperlinks referenced by `rows`. Reserved: empty until the
    /// hyperlink interner is ported.
    pub hyperlinks: Vec<Hyperlink>,
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
    /// Anchors a new selection at an explicit grid cell (mouse
    /// press).
    fn start_selection(
        &mut self,
        cell: GridPoint,
        side: CellSide,
        kind: SelectionKind,
    ) -> VtResult<Option<Damage>>;

    /// Anchors a new selection at the vi cursor (vi-mode `v` / `V`),
    /// whose position only the VT knows.
    fn start_selection_at_vi_cursor(&mut self, kind: SelectionKind) -> VtResult<Option<Damage>>;

    /// Moves the moving end of the active selection to a grid cell
    /// (mouse drag). The cell may reach into scrollback history
    /// (a negative line) when the drag leaves the viewport. `Ok(None)`
    /// when nothing is selected.
    fn update_selection(&mut self, cell: GridPoint, side: CellSide) -> VtResult<Option<Damage>>;

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
mod tests;
