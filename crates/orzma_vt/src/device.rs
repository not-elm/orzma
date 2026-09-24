//! The character-terminal device this VT emulates.

pub(crate) mod color;
pub(crate) mod cursor_policy;
pub(crate) mod modes;

use crate::device::color::{Palette, Rgb};
use crate::device::cursor_policy::CursorPolicy;
use crate::device::modes::{
    AlternateScroll, AutoWrap, CursorBlink, InsertReplaceMode, KeypadMode, ModeReport,
    MouseEncoding, MouseTracking, ScreenKind, TextCursorEnable, VtModes,
};
use crate::error::VtResult;
use crate::frame::damage::DamageSpan;
use crate::hyperlink::{HyperlinkId, HyperlinkInterner, HyperlinkUri};
use crate::placement::{InstanceId, MAX_PLACEMENTS, PlacementSize};
use crate::screen::cell::ClassifiedGlyph;
use crate::screen::cursor::Cursor;
use crate::screen::grid::coords::{GridColumn, ScreenLine};
use crate::screen::grid::reflow::ScrollbackOnGrow;
use crate::screen::grid::{GridSize, MIN_COLUMNS};
use crate::screen::margins::OriginMode;
use crate::screen::viewport::{DisplayOffset, Scroll};
use crate::screen::{PrintOptions, Screen};
use std::collections::VecDeque;

/// The emulated terminal device: screens, modes, tabs, colors, title,
/// and the terminal-scoped placement invariants.
pub(crate) struct DeviceState {
    screens: Screens,
    modes: VtModes,
    palette: Palette,
    title: TitleState,
    hyperlinks: HyperlinkInterner,
    active_hyperlink: Option<HyperlinkId>,
    preceding_graphic: Option<ClassifiedGlyph>,
    cursor_policy: CursorPolicy,
    scrollback_on_grow: ScrollbackOnGrow,
}

impl DeviceState {
    /// Builds a blank device with the primary screen active.
    ///
    /// The alternate screen is built without scrollback, so its viewport
    /// stays pinned to the live tail. Both axes of `size` must be nonzero,
    /// as [`GridSize::new`] guarantees.
    pub fn new(size: GridSize, max_history: usize) -> Self {
        Self {
            screens: Screens {
                primary: Screen::new(size, max_history),
                alternate: Screen::new(size, 0),
            },
            modes: VtModes::default(),
            palette: Palette::default(),
            title: TitleState::default(),
            hyperlinks: HyperlinkInterner::new(),
            active_hyperlink: None,
            preceding_graphic: None,
            cursor_policy: CursorPolicy::default(),
            scrollback_on_grow: ScrollbackOnGrow::default(),
        }
    }

    /// The screen the device currently reads and writes.
    pub fn active_screen(&self) -> &Screen {
        match self.modes.active_screen {
            ScreenKind::Primary => &self.screens.primary,
            ScreenKind::Alternate => &self.screens.alternate,
        }
    }

    /// The screen the device currently reads and writes.
    pub fn active_screen_mut(&mut self) -> &mut Screen {
        match self.modes.active_screen {
            ScreenKind::Primary => &mut self.screens.primary,
            ScreenKind::Alternate => &mut self.screens.alternate,
        }
    }

    /// Resizes the screens to `size`; `None` when the dimensions of the
    /// screen on show already matched or either axis of `size` is zero.
    /// A column count below [`MIN_COLUMNS`] is raised to it, as
    /// [`GridSize::new`] does.
    ///
    /// While the primary screen is shown it is reflowed with
    /// [`Screen::reflow`] under the device's [`ScrollbackOnGrow`], and the
    /// alternate screen is truncated. While the alternate screen is shown
    /// only it is resized, and the primary screen keeps its size until
    /// [`Self::switch_screen`] shows it again.
    ///
    /// Placements this strands are not named here: their anchors stop
    /// resolving, and the next [`Self::evict_lost_anchors`] names them.
    ///
    /// # Invariants
    ///
    /// A resize that changes the dimensions of the screen on show reports
    /// [`DamageSpan::Full`].
    ///
    /// Both grid axes are nonzero, and the two screens are the same size
    /// while the primary screen is shown.
    pub fn resize(&mut self, size: GridSize) -> Option<DamageSpan> {
        if size.cols == 0 || size.rows == 0 {
            return None;
        }
        let size = GridSize {
            cols: size.cols.max(MIN_COLUMNS),
            ..size
        };
        match self.modes.active_screen {
            ScreenKind::Primary => {
                let primary = self.screens.primary.reflow(size, self.scrollback_on_grow);
                let _ = self.screens.alternate.resize(size);
                primary
            }
            ScreenKind::Alternate => self.screens.alternate.resize(size),
        }
    }

    /// Moves the active viewport; `None` for a clamped or zero motion.
    ///
    /// # Invariants
    ///
    /// A motion that moves the viewport reports [`DamageSpan::Full`].
    pub fn scroll(&mut self, scroll: Scroll) -> Option<DamageSpan> {
        self.active_screen_mut().scroll(scroll)
    }

    /// Prints one character at the cursor of the screen on show, shaped by
    /// the device's `IRM` and `DECAWM` modes and its open hyperlink, after
    /// mapping it through that screen's character set mapping.
    ///
    /// A control character is ignored and leaves a pending single shift
    /// pending; any other character spends it, even a zero-width mark or a
    /// character the screen drops.
    ///
    /// A mapped character one or two columns wide becomes the
    /// [`Self::preceding_graphic`], even when the screen drops it instead of
    /// placing it. A zero-width mark and a character with no width leave the
    /// preceding graphic character as it was.
    ///
    /// Reports [`DamageSpan::Full`] when a wrap scrolled, otherwise every
    /// row the character touched, or `None` when nothing changed or those
    /// rows have scrolled out of the window.
    ///
    /// # Errors
    ///
    /// [`VtError::Stamp`](crate::error::VtError::Stamp) when the row
    /// refuses the character.
    pub fn print(&mut self, c: char) -> VtResult<Option<DamageSpan>> {
        // NOTE: A control character is dropped before the mapping so that it
        // cannot spend a pending single shift — only a graphic character may
        // do that.
        if c.is_control() {
            return Ok(None);
        }
        let Some(glyph) = ClassifiedGlyph::classify(self.active_screen_mut().translate(c)) else {
            return Ok(None);
        };
        if glyph.class().body_width().is_some() {
            self.preceding_graphic = Some(glyph);
        }
        self.print_graphic(glyph)
    }

    /// The graphic character `REP` repeats: the last character one or two
    /// columns wide that [`Self::print`] mapped, or `None` when it has mapped
    /// none since power-up or the last `RIS`.
    ///
    /// Control functions, a soft reset, and a flip between the screens leave
    /// it as it is; only [`Self::reset`] clears it.
    pub fn preceding_graphic(&self) -> Option<ClassifiedGlyph> {
        self.preceding_graphic
    }

    /// Prints `glyph`, already mapped through a character set, at the cursor
    /// of the screen on show, shaped by the device's `IRM` and `DECAWM`
    /// modes, its open hyperlink, and the screen's current pen.
    ///
    /// A pending single shift stays pending, and the preceding graphic
    /// character is left as it was.
    ///
    /// Reports [`DamageSpan::Full`] when a wrap scrolled, otherwise every
    /// row the glyph touched, or `None` when nothing changed or those rows
    /// have scrolled out of the window.
    ///
    /// # Errors
    ///
    /// [`VtError::Stamp`](crate::error::VtError::Stamp) when the row
    /// refuses the glyph.
    pub fn print_graphic(&mut self, glyph: ClassifiedGlyph) -> VtResult<Option<DamageSpan>> {
        // NOTE: Every field is spelled out so that a field added to
        // `PrintOptions` fails to compile here instead of silently printing
        // with its default.
        let options = PrintOptions {
            insert_replace: self.modes.insert_replace,
            auto_wrap: self.modes.auto_wrap,
            hyperlink_id: self.active_hyperlink,
        };
        self.active_screen_mut().print(glyph, options)
    }

    /// Returns both screens and every mode to their power-up state;
    /// `None` when the frame that follows needs no repaint.
    ///
    /// Only the screen left active by the mode reset reaches a frame, so
    /// the alternate screen's damage is dropped rather than folded in.
    /// A reset that arrives while the alternate screen is shown always
    /// repaints.
    ///
    /// The title is cleared too, dropping both the current title and the
    /// whole save stack.
    ///
    /// The whole palette returns to its built-in defaults too, and a
    /// reset that changes a color reports a full repaint even when
    /// neither screen was written.
    ///
    /// The hyperlinks a program opened stay resolvable, though the open
    /// one is closed.
    ///
    /// The preceding graphic character is cleared, so a `REP` that follows
    /// prints nothing.
    ///
    /// The cursor's shape and blink return to the host-supplied cursor
    /// policy's initial style rather than the power-up one.
    ///
    /// The primary screen comes back at the size the alternate screen has.
    ///
    /// # Control Functions
    ///
    /// - `RIS` (`ESC c`)
    pub fn reset(&mut self) -> Option<DamageSpan> {
        self.preceding_graphic = None;
        let size = self.screens.alternate.grid_size();
        let _ = self.screens.primary.resize(size);
        let was_showing_alternate = matches!(self.modes.active_screen, ScreenKind::Alternate);
        let primary = self.screens.primary.reset();
        let _ = self.screens.alternate.reset();
        // NOTE: This wholesale write is the one place `auto_wrap` is set
        // without `DeviceState::set_auto_wrap`, and it is sound only because
        // the two screen resets above already cleared each screen's
        // live and saved deferred wrap. A partial mode reset such as
        // `DECSTR`, which touches only the screen on show and leaves its
        // live deferred wrap as it stands, must go through `set_auto_wrap`
        // instead.
        self.modes = VtModes::default();
        self.apply_initial_cursor_style();
        self.title = TitleState::default();
        self.active_hyperlink = None;
        // NOTE: `hyperlinks` is deliberately not reset. Ids must never be
        // reused: the renderer keeps its own id-to-uri table and skips an
        // id it already knows, so a reused id would resolve to the uri it
        // carried before the reset.
        let palette_changed = self.palette.reset();
        (was_showing_alternate || primary.is_some() || palette_changed).then_some(DamageSpan::Full)
    }

    /// Returns the modes a soft reset names, the scrolling margins,
    /// cursor origin, character set mapping, pen and saved cursor of the
    /// screen on show, and every indexed palette slot to their power-up
    /// values; reports [`DamageSpan::Full`] when a palette slot changed.
    ///
    /// Autowrap returns to enabled, which is the set rather than the
    /// reset state vt510.pdf p.277 Table 5-9 lists.
    ///
    /// The open hyperlink is closed.
    ///
    /// The modes it does not name are left as they are, and so are the
    /// cells and the cursor position on show, the hidden screen, the
    /// title, and the palette's foreground, background, and cursor
    /// color.
    ///
    /// The cursor's shape and blink are left as they are; vt510.pdf
    /// p.277 Table 5-9 lists only `Text cursor enable`.
    ///
    /// # Control Functions
    ///
    /// - `DECSTR` (`CSI ! p`)
    pub fn soft_reset(&mut self) -> Option<DamageSpan> {
        self.modes.text_cursor.enable = TextCursorEnable::Shown;
        self.modes.insert_replace = InsertReplaceMode::Replace;
        self.modes.app_cursor = false;
        self.modes.keypad_mode = KeypadMode::Numeric;
        self.set_auto_wrap(AutoWrap::Enabled);
        self.active_screen_mut().soft_reset();
        self.active_hyperlink = None;
        self.reset_indexed_colors().then_some(DamageSpan::Full)
    }

    /// The window title the application last set, if any.
    pub fn title(&self) -> Option<&str> {
        self.title.current.as_deref()
    }

    /// Replaces the window title; `None` returns it to the host's
    /// default.
    ///
    /// # Control Functions
    ///
    /// - `OSC 0` / `OSC 2`
    pub fn set_title(&mut self, title: Option<String>) {
        self.title.current = title;
    }

    /// Saves the current title on the stack, dropping the oldest entry
    /// once the stack is full.
    ///
    /// # Control Functions
    ///
    /// - `XTWINOPS` (`CSI 22 t`); the icon/window selector and the
    ///   direct slot number xterm accepts after it are both ignored, so
    ///   a slot store arrives here as an ordinary push.
    pub fn push_title(&mut self) {
        if self.title.stack.len() == MAX_TITLE_DEPTH {
            self.title.stack.pop_front();
        }
        self.title.stack.push_back(self.title.current.clone());
    }

    /// Takes the most recently saved title off the stack without
    /// applying it; `None` when the stack was empty.
    ///
    /// The two layers of `Option` mean different things: the outer one
    /// reports whether the stack held anything at all, and the inner one
    /// whether the entry it held was a title or the absence of one.
    ///
    /// # Control Functions
    ///
    /// - `XTWINOPS` (`CSI 23 t`); the icon/window selector and the
    ///   direct slot number xterm accepts after it are both ignored, so
    ///   a slot fetch arrives here as an ordinary pop.
    pub fn pop_title(&mut self) -> Option<Option<String>> {
        self.title.stack.pop_back()
    }

    /// Returns the grid dimensions of the active screen.
    pub fn grid_size(&self) -> GridSize {
        self.active_screen().grid_size()
    }

    /// Scrollback rows the active viewport sits above the live tail.
    pub fn display_offset(&self) -> DisplayOffset {
        self.active_screen().display_offset()
    }

    /// The cursor an emitted frame carries: the active screen's write
    /// position with this device's cursor presentation folded in.
    ///
    /// A frame-ready cursor must be read through here rather than by
    /// pairing a screen read with a separately-read mode.
    pub fn cursor(&self) -> Cursor {
        self.active_screen().cursor(self.modes.text_cursor)
    }

    /// Snapshot of the modes the device owns.
    pub fn modes(&self) -> VtModes {
        self.modes
    }

    /// Mutably borrows the modes the device owns.
    pub fn modes_mut(&mut self) -> &mut VtModes {
        &mut self.modes
    }

    /// The DECRPM value for the DEC private mode `mode`.
    ///
    /// A mode this terminal keeps no state for reports
    /// [`ModeReport::NotRecognized`].
    ///
    /// Mode 6 reports the screen on show, and modes 47, 1047 and 1049
    /// all report whether the alternate screen is shown. Modes 1000,
    /// 1002 and 1003 report set only for the tracking level in force.
    /// Mode 12 reports the blink `DECSCUSR` selected as well.
    ///
    /// # Control Functions
    ///
    /// - `DECRQM` (`CSI ? Ps $ p`)
    pub fn private_mode_report(&self, mode: u16) -> ModeReport {
        let modes = self.modes;
        let set = match mode {
            1 => modes.app_cursor,
            6 => self.active_screen().origin_mode() == OriginMode::WithinMargins,
            7 => modes.auto_wrap.wraps(),
            12 => modes.text_cursor.blink == CursorBlink::Blinking,
            25 => modes.text_cursor.enable == TextCursorEnable::Shown,
            47 | 1047 | 1049 => modes.active_screen == ScreenKind::Alternate,
            66 => modes.keypad_mode == KeypadMode::Application,
            1000 => modes.mouse_tracking == MouseTracking::Clicks,
            1002 => modes.mouse_tracking == MouseTracking::Drag,
            1003 => modes.mouse_tracking == MouseTracking::Motion,
            1004 => modes.focus_in_out,
            1006 => modes.mouse_encoding == MouseEncoding::Sgr,
            1007 => modes.alternate_scroll == AlternateScroll::Enabled,
            2004 => modes.bracketed_paste,
            2026 => modes.synchronized_output.is_active(),
            _ => return ModeReport::NotRecognized,
        };
        ModeReport::from_flag(set)
    }

    /// The DECRPM value for the ANSI mode `mode`; only insert mode (4)
    /// is recognized.
    ///
    /// # Control Functions
    ///
    /// - `DECRQM` (`CSI Ps $ p`)
    pub fn ansi_mode_report(&self, mode: u16) -> ModeReport {
        match mode {
            4 => ModeReport::from_flag(self.modes.insert_replace == InsertReplaceMode::Insert),
            _ => ModeReport::NotRecognized,
        }
    }

    /// Applies `DECAWM`, disarming each screen's deferred wrap when the
    /// mode is reset.
    ///
    /// The mode is device-wide, so a reset disarms both screens rather
    /// than only the one shown. A set leaves an armed flag alone: DEC
    /// STD-070 lists only the reset direction among the operations that
    /// clear the last-column flag.
    ///
    /// The disarm does not reach a checkpoint, so a later restore
    /// (`DECRC`, 1048, 1049) puts the saved flag back.
    ///
    /// # Control Functions
    ///
    /// - `DECAWM` (`CSI ? 7 h` / `CSI ? 7 l`)
    pub fn set_auto_wrap(&mut self, auto_wrap: AutoWrap) {
        self.modes.auto_wrap = auto_wrap;
        if !auto_wrap.wraps() {
            self.screens.primary.disarm_pending_wrap();
            self.screens.alternate.disarm_pending_wrap();
        }
    }

    /// The live palette symbolic colors resolve against.
    pub fn palette(&self) -> &Palette {
        &self.palette
    }

    /// Sets palette slot `index` to `color`; returns whether the slot
    /// changed.
    ///
    /// # Control Functions
    ///
    /// - `OSC 4 ; c ; spec`
    pub fn set_indexed_color(&mut self, index: u8, color: Rgb) -> bool {
        self.palette.set_indexed(index, color)
    }

    /// Returns palette slot `index` to its xterm default; returns
    /// whether the slot changed.
    ///
    /// # Control Functions
    ///
    /// - `OSC 104 ; c`
    pub fn reset_indexed_color(&mut self, index: u8) -> bool {
        self.palette.reset_indexed(index)
    }

    /// Returns every palette slot to its xterm default; returns whether
    /// any slot changed.
    ///
    /// # Control Functions
    ///
    /// - `OSC 104` with no colour number
    pub fn reset_indexed_colors(&mut self) -> bool {
        self.palette.reset_all_indexed()
    }

    /// Opens a hyperlink, so the cells printed from now on carry it. A
    /// nonempty `id` joins this link to every other open naming the same
    /// id and uri.
    ///
    /// # Control Functions
    ///
    /// - `OSC 8 ; params ; URI`
    pub fn open_hyperlink(&mut self, id: Option<String>, uri: HyperlinkUri) {
        self.active_hyperlink = Some(self.hyperlinks.open(id, uri));
    }

    /// Closes the open hyperlink, so the cells printed from now on carry
    /// none. The cells already printed keep theirs.
    ///
    /// # Control Functions
    ///
    /// - `OSC 8 ; ;`
    pub fn close_hyperlink(&mut self) {
        self.active_hyperlink = None;
    }

    /// The target `id` was opened for, or `None` for an id this device
    /// never handed out.
    pub fn hyperlink_uri(&self, id: HyperlinkId) -> Option<&HyperlinkUri> {
        self.hyperlinks.extract(&id)
    }

    /// Sets the default foreground to `color`; returns whether it
    /// changed.
    ///
    /// # Control Functions
    ///
    /// - `OSC 10 ; spec`
    pub fn set_foreground_color(&mut self, color: Rgb) -> bool {
        self.palette.set_foreground(color)
    }

    /// Sets the default background to `color`; returns whether it
    /// changed.
    ///
    /// # Control Functions
    ///
    /// - `OSC 11 ; spec`
    pub fn set_background_color(&mut self, color: Rgb) -> bool {
        self.palette.set_background(color)
    }

    /// Returns the default foreground to its built-in default; returns
    /// whether it changed.
    ///
    /// # Control Functions
    ///
    /// - `OSC 110`
    pub fn reset_foreground_color(&mut self) -> bool {
        self.palette.reset_foreground()
    }

    /// Returns the default background to its built-in default; returns
    /// whether it changed.
    ///
    /// # Control Functions
    ///
    /// - `OSC 111`
    pub fn reset_background_color(&mut self) -> bool {
        self.palette.reset_background()
    }

    /// Sets the text cursor color to `color`; returns whether it
    /// changed.
    ///
    /// # Control Functions
    ///
    /// - `OSC 12 ; spec`
    pub fn set_cursor_color(&mut self, color: Rgb) -> bool {
        self.palette.set_cursor(color)
    }

    /// Returns the text cursor color to unset; returns whether it
    /// changed.
    ///
    /// # Control Functions
    ///
    /// - `OSC 112`
    pub fn reset_cursor_color(&mut self) -> bool {
        self.palette.reset_cursor()
    }

    /// The host-supplied cursor policy this device applies.
    pub fn cursor_policy(&self) -> CursorPolicy {
        self.cursor_policy
    }

    /// Replaces the host-supplied cursor policy and applies its initial
    /// style at once, leaving the cursor's visibility untouched.
    pub fn set_cursor_policy(&mut self, policy: CursorPolicy) {
        self.cursor_policy = policy;
        self.apply_initial_cursor_style();
    }

    /// Sets what a resize does with the rows it frees at the bottom of the
    /// primary screen.
    pub fn set_scrollback_on_grow(&mut self, policy: ScrollbackOnGrow) {
        self.scrollback_on_grow = policy;
    }

    /// Switches the active screen without a flip's side effects.
    #[cfg(test)]
    pub(crate) fn set_active_screen_for_test(&mut self, kind: ScreenKind) {
        self.modes.active_screen = kind;
    }

    fn apply_initial_cursor_style(&mut self) {
        self.modes.text_cursor = self
            .modes
            .text_cursor
            .with_style(self.cursor_policy.initial);
    }
}

/// Webview placements.
///
/// The cap counts both screens, and a live id is unique across the
/// pair.
impl DeviceState {
    /// Registers a mount at the active screen's cursor under the id the
    /// host minted; `false` when the cap rejects it.
    ///
    /// # Invariants
    ///
    /// A re-mount of a live id frees the slot it takes, so it succeeds
    /// even at the cap.
    pub fn mount_placement(&mut self, size: PlacementSize, id: InstanceId) -> bool {
        self.supersede_placement(id);
        if MAX_PLACEMENTS <= self.placement_count() {
            return false;
        }
        self.active_screen_mut().mount_placement(id, size);
        true
    }

    /// Registers a mount anchored at the active screen's visible cell
    /// (`row`, `column`) under the id the host minted; `false` when the
    /// cell lies outside the grid or the cap rejects it.
    ///
    /// # Invariants
    ///
    /// A re-mount of a live id frees the slot it takes, so it succeeds
    /// even at the cap.
    pub fn mount_placement_at(
        &mut self,
        row: ScreenLine,
        column: GridColumn,
        size: PlacementSize,
        id: InstanceId,
    ) -> bool {
        // NOTE: the bounds check must precede supersession. `Grid::line_id`
        // indexes the ring unchecked and panics on a row past the grid, and
        // supersession drops the live placement under `id`, so a rejected
        // re-mount must return here and leave that placement untouched.
        let grid = self.active_screen().grid_size();
        if grid.rows <= row.0 || grid.cols <= column.0 {
            return false;
        }
        self.supersede_placement(id);
        if MAX_PLACEMENTS <= self.placement_count() {
            return false;
        }
        self.active_screen_mut()
            .mount_placement_at(id, row, column, size);
        true
    }

    /// Removes the placement a client `unmount` addresses on either
    /// screen; returns whether anything went.
    ///
    /// An unmount-all removes the matching placements on both screens.
    pub fn unmount_placement(&mut self, id: Option<InstanceId>) -> bool {
        let primary = self.screens.primary.unmount_placement(id);
        let alternate = self.screens.alternate.unmount_placement(id);
        primary || alternate
    }

    /// Removes the placements the host names on either screen; returns
    /// whether anything went.
    pub fn remove_placements(&mut self, ids: &[InstanceId]) -> bool {
        let primary = self.screens.primary.remove_placements(ids);
        let alternate = self.screens.alternate.remove_placements(ids);
        primary || alternate
    }

    /// Sweeps both screens for placements whose anchor no longer resolves
    /// and names them.
    ///
    /// # Invariants
    ///
    /// The primary screen's ids come first.
    pub fn evict_lost_anchors(&mut self) -> Vec<InstanceId> {
        let mut evicted = self.screens.primary.evict_lost_anchors();
        evicted.extend(self.screens.alternate.evict_lost_anchors());
        evicted
    }

    /// Applies an alternate-screen flip, tearing down the placements and
    /// the selection the abandoned alternate screen owned.
    ///
    /// Primary placements and the primary selection are hidden while the
    /// alternate screen is shown, not destroyed. This operation stages no
    /// damage of its own: the caller must stage `DamageSpan::Full` for
    /// the flip.
    ///
    /// A flip in either direction closes the open hyperlink. Each screen
    /// keeps its own pen, colours included, across flips.
    ///
    /// A flip to the primary screen first reflows it to the alternate
    /// screen's size when a resize arrived while the alternate screen was
    /// shown. The placements that reflow strands are not named here: their
    /// anchors stop resolving, and the next [`Self::evict_lost_anchors`]
    /// names them.
    pub fn switch_screen(&mut self, to: ScreenKind) -> Vec<InstanceId> {
        self.modes.active_screen = to;
        // NOTE: Dropping this clear lets a link a killed program left open
        // cover every cell the next program prints after the flip. It runs
        // only on a real flip, so a repeated set on the screen already
        // shown leaves such a link open.
        self.active_hyperlink = None;
        match to {
            ScreenKind::Alternate => Vec::new(),
            ScreenKind::Primary => {
                self.screens.alternate.clear_selection();
                let evicted = self.screens.alternate.take_placements();
                let size = self.screens.alternate.grid_size();
                let _ = self.screens.primary.reflow(size, self.scrollback_on_grow);
                evicted
            }
        }
    }

    fn supersede_placement(&mut self, id: InstanceId) {
        self.screens.primary.supersede_placement(id);
        self.screens.alternate.supersede_placement(id);
    }

    /// Live placements across both screens — what the cap counts.
    fn placement_count(&self) -> usize {
        self.screens.primary.placement_count() + self.screens.alternate.placement_count()
    }
}

/// The primary / alternate pair.
struct Screens {
    primary: Screen,
    alternate: Screen,
}

/// The current window title and the stack `CSI 22 t` saves it on.
#[derive(Default)]
struct TitleState {
    current: Option<String>,
    stack: VecDeque<Option<String>>,
}

/// Titles `CSI 22 t` may stack before the oldest is dropped.
///
/// xterm documents direct stack access over slots 1 through 10, which
/// this bound covers.
const MAX_TITLE_DEPTH: usize = 16;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::color::{Color, Rgb};
    use crate::device::modes::{
        AlternateScroll, InsertReplaceMode, KeypadMode, MouseEncoding, MouseTracking,
        TextCursorEnable,
    };
    use crate::hyperlink::HyperlinkUri;
    use crate::screen::cell::Cell;
    use crate::screen::character_sets::{CharacterSet, GCode};
    use crate::screen::grid::coords::{GridColumn, GridLine};
    use crate::screen::grid::reflow::ScrollbackOnGrow;
    use crate::screen::viewport::ViewportLine;
    use std::iter::from_fn;

    fn device() -> DeviceState {
        DeviceState::new(GridSize { cols: 8, rows: 3 }, 10)
    }

    fn mount(device: &mut DeviceState, id: InstanceId) -> bool {
        device.mount_placement(PlacementSize { rows: 2, cols: 4 }, id)
    }

    /// Asserts that opening a hyperlink makes its id the active one.
    ///
    /// Case: a build tool starts printing a clickable path.
    #[test]
    fn opening_a_hyperlink_makes_it_active() {
        let mut device = device();
        device.open_hyperlink(None, HyperlinkUri::new("https://a.example"));
        assert!(device.active_hyperlink.is_some());
    }

    /// Asserts that closing a hyperlink leaves none active.
    ///
    /// Case: a program finishes printing a link and emits the closing
    /// sequence before its next word.
    #[test]
    fn closing_a_hyperlink_leaves_none_active() {
        let mut device = device();
        device.open_hyperlink(None, HyperlinkUri::new("https://a.example"));
        device.close_hyperlink();
        assert_eq!(device.active_hyperlink, None);
    }

    /// Asserts that a full reset leaves no hyperlink active.
    ///
    /// Case: a program leaves a link open and the user runs `reset`.
    #[test]
    fn a_reset_leaves_no_hyperlink_active() {
        let mut device = device();
        device.open_hyperlink(None, HyperlinkUri::new("https://a.example"));
        let _ = device.reset();
        assert_eq!(device.active_hyperlink, None);
    }

    /// Asserts that a soft reset leaves no hyperlink active.
    ///
    /// Case: a program leaves a link open and a later `tput init` issues a
    /// soft reset.
    #[test]
    fn a_soft_reset_leaves_no_hyperlink_active() {
        let mut device = device();
        device.open_hyperlink(None, HyperlinkUri::new("https://a.example"));
        let _ = device.soft_reset();
        assert_eq!(device.active_hyperlink, None);
    }

    /// Asserts that a flip in either direction leaves no hyperlink active.
    ///
    /// Case: a shell leaves a link open when a full-screen editor starts,
    /// and the editor leaves one open when it exits.
    #[test]
    fn a_screen_flip_in_either_direction_leaves_no_hyperlink_active() {
        let mut device = device();
        for to in [ScreenKind::Alternate, ScreenKind::Primary] {
            device.open_hyperlink(None, HyperlinkUri::new("https://a.example"));
            let _ = device.switch_screen(to);
            assert_eq!(device.active_hyperlink, None, "flipped to {to:?}");
        }
    }

    /// Asserts that a scroll moves the screen on show and leaves the
    /// other one where it was.
    ///
    /// Case: the user scrolls back through shell output, then a
    /// full-screen editor takes over the alternate screen.
    #[test]
    fn a_scroll_moves_only_the_screen_on_show() {
        let mut device = DeviceState::new(GridSize { cols: 4, rows: 3 }, 10);
        for _ in 0..5 {
            device.active_screen_mut().move_cursor_to(Some(3), None);
            device.active_screen_mut().line_feed();
        }
        assert_eq!(device.scroll(Scroll::Top), Some(DamageSpan::Full));
        assert_eq!(device.display_offset(), DisplayOffset(5));
        device.switch_screen(ScreenKind::Alternate);
        assert_eq!(device.display_offset(), DisplayOffset(0));
    }

    /// Asserts that a scroll on the alternate screen reports nothing.
    ///
    /// Case: the user rolls the wheel while a full-screen editor is
    /// showing and alternate-scroll translation is off.
    #[test]
    fn a_scroll_on_the_alternate_screen_reports_nothing() {
        let mut device = DeviceState::new(GridSize { cols: 4, rows: 3 }, 10);
        device.switch_screen(ScreenKind::Alternate);
        assert_eq!(device.scroll(Scroll::Top), None);
        assert_eq!(device.scroll(Scroll::PageUp), None);
        assert_eq!(device.display_offset(), DisplayOffset(0));
    }

    /// Asserts that a resize to the size the device already has
    /// reports no damage.
    ///
    /// Case: the window manager replays the same geometry after a
    /// focus change, so the host forwards a size the VT already holds.
    #[test]
    fn a_resize_to_the_current_size_reports_nothing() {
        let mut device = DeviceState::new(GridSize { cols: 4, rows: 3 }, 10);
        assert_eq!(device.resize(GridSize { cols: 4, rows: 3 }), None);
    }

    /// Asserts that a resize reports full damage and applies to both
    /// screens, not only the one on show.
    ///
    /// Case: the user resizes the window while a full-screen editor is
    /// running, then quits it back to the shell.
    #[test]
    fn a_resize_applies_to_both_screens() {
        let mut device = DeviceState::new(GridSize { cols: 4, rows: 3 }, 10);
        assert_eq!(
            device.resize(GridSize { cols: 8, rows: 5 }),
            Some(DamageSpan::Full)
        );
        assert_eq!(
            device.active_screen().grid_size(),
            GridSize { cols: 8, rows: 5 }
        );
        device.switch_screen(ScreenKind::Alternate);
        assert_eq!(
            device.active_screen().grid_size(),
            GridSize { cols: 8, rows: 5 }
        );
    }

    /// Asserts that a shrink deep enough to drop an anchor row out of
    /// history leaves that placement evictable.
    ///
    /// Case: a webview is mounted near the top of a short-scrollback
    /// window and the user drags the window much shorter.
    #[test]
    fn a_shrink_past_the_history_cap_leaves_its_placements_evictable() {
        let mut device = DeviceState::new(GridSize { cols: 4, rows: 4 }, 0);
        let id = InstanceId(1);
        assert!(mount(&mut device, id));
        device.active_screen_mut().move_cursor_to(Some(4), None);
        assert_eq!(
            device.resize(GridSize { cols: 4, rows: 2 }),
            Some(DamageSpan::Full)
        );
        assert_eq!(device.evict_lost_anchors(), vec![id]);
    }

    fn row_text(device: &DeviceState, line: i32) -> String {
        device
            .screens
            .primary
            .grid()
            .row(GridLine(line))
            .iter()
            .flat_map(Cell::chars)
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    /// Asserts that a resize while the alternate screen is shown leaves
    /// the primary screen at its size until the flip back reflows it.
    ///
    /// Case: the user narrows the window while a full-screen editor is
    /// open, then quits the editor.
    #[test]
    fn the_primary_screen_reflows_only_when_shown_again() {
        let mut device = DeviceState::new(GridSize { cols: 8, rows: 3 }, 10);
        for c in "abcdefg".chars() {
            device.print(c).expect("a printable glyph");
        }
        let _ = device.switch_screen(ScreenKind::Alternate);
        assert_eq!(
            device.resize(GridSize { cols: 4, rows: 3 }),
            Some(DamageSpan::Full)
        );
        assert_eq!(
            device.screens.primary.grid_size(),
            GridSize { cols: 8, rows: 3 }
        );
        let _ = device.switch_screen(ScreenKind::Primary);
        assert_eq!(
            device.active_screen().grid_size(),
            GridSize { cols: 4, rows: 3 }
        );
        assert_eq!(row_text(&device, 0), "abcd");
        assert_eq!(row_text(&device, 1), "efg");
    }

    /// Asserts that several resizes behind the alternate screen reflow the
    /// primary screen once, from its old size to the last one.
    ///
    /// Case: the user drags the window narrower and back while a
    /// full-screen editor is open under ConPTY.
    #[test]
    fn resizes_behind_the_alternate_screen_collapse_into_one_reflow() {
        let mut device = DeviceState::new(GridSize { cols: 4, rows: 2 }, 10);
        device.set_scrollback_on_grow(ScrollbackOnGrow::Keep);
        for c in "abcdefgh".chars() {
            device.print(c).expect("a printable glyph");
        }
        let _ = device.switch_screen(ScreenKind::Alternate);
        let _ = device.resize(GridSize { cols: 2, rows: 2 });
        let _ = device.resize(GridSize { cols: 4, rows: 2 });
        let _ = device.switch_screen(ScreenKind::Primary);
        assert_eq!(device.screens.primary.grid().history_len(), 0);
    }

    /// Asserts that a reset while the alternate screen is shown brings the
    /// primary screen back at the current size.
    ///
    /// Case: a program sends `RIS` from a full-screen editor after the user
    /// resized the window.
    #[test]
    fn a_reset_on_the_alternate_screen_brings_the_primary_back_at_the_current_size() {
        let mut device = DeviceState::new(GridSize { cols: 8, rows: 3 }, 10);
        let _ = device.switch_screen(ScreenKind::Alternate);
        let _ = device.resize(GridSize { cols: 4, rows: 5 });
        let _ = device.reset();
        assert_eq!(
            device.active_screen().grid_size(),
            GridSize { cols: 4, rows: 5 }
        );
    }

    /// Asserts that a size with a zero axis is ignored rather than
    /// applied.
    ///
    /// Case: a host builds the size from a minimized window's geometry
    /// without going through `GridSize::new`.
    #[test]
    fn a_zero_axis_resize_is_ignored() {
        let mut device = DeviceState::new(GridSize { cols: 8, rows: 3 }, 10);
        assert_eq!(device.resize(GridSize { cols: 0, rows: 3 }), None);
        assert_eq!(device.resize(GridSize { cols: 8, rows: 0 }), None);
        assert_eq!(
            device.active_screen().grid_size(),
            GridSize { cols: 8, rows: 3 }
        );
    }

    /// Asserts that a one-column size is widened to the narrowest grid
    /// rather than applied.
    ///
    /// Case: a host builds a one-column size from a sliver of a pane
    /// without going through `GridSize::new`.
    #[test]
    fn a_one_column_resize_is_widened_to_the_narrowest_grid() {
        let mut device = DeviceState::new(GridSize { cols: 8, rows: 3 }, 10);
        let _ = device.resize(GridSize { cols: 1, rows: 3 });
        assert_eq!(
            device.active_screen().grid_size(),
            GridSize {
                cols: MIN_COLUMNS,
                rows: 3
            }
        );
    }

    /// Asserts that a stop set on one screen is absent from the other,
    /// each screen keeping its own tab stop table rather than sharing one.
    ///
    /// Case: a shell installs its own tab positions, then a full-screen
    /// editor takes over the alternate screen and emits a tab.
    #[test]
    fn the_two_screens_carry_independent_tab_stops() {
        let mut device = DeviceState::new(GridSize { cols: 20, rows: 3 }, 10);
        for c in ['a', 'b', 'c'] {
            device.print(c).expect("a printable glyph");
        }
        device.active_screen_mut().set_horizontal_tab_stop();

        device.set_active_screen_for_test(ScreenKind::Alternate);
        device.active_screen_mut().move_forward_tabs(1);
        assert_eq!(device.active_screen().cursor_column(), GridColumn(8));

        device.set_active_screen_for_test(ScreenKind::Primary);
        device.active_screen_mut().carriage_return();
        device.active_screen_mut().move_forward_tabs(1);
        assert_eq!(device.active_screen().cursor_column(), GridColumn(3));
    }

    /// Asserts that a checkpoint saved on one screen is unreachable from
    /// the other, so a restore on the alternate screen returns its own
    /// power-up state instead of consuming the save the shell left on the
    /// primary.
    ///
    /// Case: a shell saves its cursor, a full-screen editor takes over
    /// the alternate screen and emits a restore of its own, and the
    /// shell then restores after the editor exits.
    #[test]
    fn the_two_screens_carry_independent_checkpoints() {
        let mut device = DeviceState::new(GridSize { cols: 20, rows: 3 }, 10);
        for c in ['a', 'b', 'c'] {
            device.print(c).expect("a printable glyph");
        }
        device.active_screen_mut().save_checkpoint();

        device.set_active_screen_for_test(ScreenKind::Alternate);
        device.print('x').expect("a printable glyph");
        device.active_screen_mut().restore_checkpoint();
        assert_eq!(device.active_screen().cursor_column(), GridColumn(0));

        device.set_active_screen_for_test(ScreenKind::Primary);
        device.active_screen_mut().carriage_return();
        device.active_screen_mut().restore_checkpoint();
        assert_eq!(device.active_screen().cursor_column(), GridColumn(3));
    }

    /// Asserts that a reset clears both screens rather than only the one
    /// the device is showing.
    ///
    /// Case: a full-screen editor leaves its buffer on the alternate
    /// screen, the user quits back to the shell, and the shell then
    /// sends `ESC c`.
    #[test]
    fn a_reset_clears_both_screens() {
        let mut device = device();
        device.print('p').expect("a printable glyph");
        device.set_active_screen_for_test(ScreenKind::Alternate);
        device.print('a').expect("a printable glyph");

        let _ = device.reset();

        for kind in [ScreenKind::Primary, ScreenKind::Alternate] {
            device.set_active_screen_for_test(kind);
            let row = device.active_screen().viewport_row(ViewportLine(0));
            assert!(row.iter().all(|cell| *cell == Cell::default()));
        }
    }

    /// Asserts that a reset returns the device to the primary screen.
    ///
    /// Case: a full-screen application is killed while it still holds
    /// the alternate screen, and the user runs `reset` to get a usable
    /// shell back.
    #[test]
    fn a_reset_returns_the_device_to_the_primary_screen() {
        let mut device = device();
        device.set_active_screen_for_test(ScreenKind::Alternate);

        let _ = device.reset();

        assert_eq!(device.modes().active_screen, ScreenKind::Primary);
    }

    /// Asserts that a reset of an untouched device reports no repaint.
    ///
    /// Case: a login script runs `tput reset` before anything has been
    /// printed to the terminal.
    #[test]
    fn a_reset_of_an_untouched_device_reports_no_repaint() {
        assert_eq!(device().reset(), None);
    }

    /// Asserts that a reset reports a full repaint for the content the
    /// primary screen carried.
    ///
    /// Case: a program leaves garbage across the shell's screen and the
    /// user runs `reset` to clear it.
    #[test]
    fn a_reset_of_a_written_primary_screen_reports_a_full_repaint() {
        let mut device = device();
        device.print('x').expect("a printable glyph");

        assert_eq!(device.reset(), Some(DamageSpan::Full));
    }

    /// Asserts that a reset reports a full repaint whenever the
    /// alternate screen was showing, even with nothing on the primary
    /// screen behind it.
    ///
    /// Case: a full-screen application that never wrote to the primary
    /// screen is reset while it still holds the alternate screen.
    #[test]
    fn a_reset_of_a_shown_alternate_screen_reports_a_full_repaint() {
        let mut device = device();
        device.set_active_screen_for_test(ScreenKind::Alternate);

        assert_eq!(device.reset(), Some(DamageSpan::Full));
    }

    /// Asserts that a reset does not report the hidden screen's damage.
    ///
    /// Case: a full-screen editor leaves its buffer on the alternate
    /// screen, the user quits back to a shell screen that nothing has
    /// been printed to, and a startup script then sends `ESC c`.
    #[test]
    fn a_reset_does_not_report_the_hidden_screens_damage() {
        let mut device = device();
        device.set_active_screen_for_test(ScreenKind::Alternate);
        device.print('x').expect("a printable glyph");
        device.set_active_screen_for_test(ScreenKind::Primary);

        assert_eq!(device.reset(), None);
    }

    /// Asserts that the cap counts both screens, so a mount is rejected
    /// once the pair holds `MAX_PLACEMENTS` between them.
    ///
    /// Case: a program fills the primary screen with webviews, flips to
    /// the alternate screen, and keeps mounting there.
    #[test]
    fn a_mount_at_the_cap_is_rejected_across_both_screens() {
        let mut device = device();
        let per_screen = MAX_PLACEMENTS / 2;
        for index in 0..per_screen {
            assert!(mount(&mut device, InstanceId(index as u128)));
        }
        device.set_active_screen_for_test(ScreenKind::Alternate);
        for index in 0..MAX_PLACEMENTS - per_screen {
            assert!(mount(&mut device, InstanceId(1000 + index as u128)));
        }
        assert!(!mount(&mut device, InstanceId(9999)));
    }

    /// Asserts that a re-mount of the same id supersedes across the
    /// screen pair rather than leaving a twin on the other screen.
    ///
    /// Case: a program mounts a view on the primary screen, flips
    /// to the alternate screen, and re-mounts the same id there.
    #[test]
    fn a_remount_supersedes_across_screens() {
        let mut device = device();
        let id = InstanceId(1);
        assert!(mount(&mut device, id));
        device.set_active_screen_for_test(ScreenKind::Alternate);
        assert!(mount(&mut device, id));
        assert_eq!(device.placement_count(), 1);
    }

    /// Asserts that a broad unmount reaches both screens rather than
    /// stopping at the first match.
    ///
    /// Case: a program mounted a view on each screen and exits, so the
    /// host despawns every child across the terminal in one pass.
    #[test]
    fn a_broad_unmount_reaches_both_screens() {
        let mut device = device();
        assert!(mount(&mut device, InstanceId(1)));
        device.set_active_screen_for_test(ScreenKind::Alternate);
        assert!(mount(&mut device, InstanceId(2)));
        assert!(device.unmount_placement(None));
        assert_eq!(device.placement_count(), 0);
    }

    /// Asserts that a host removal naming one instance per screen clears
    /// both rather than stopping at the first match, and reports that
    /// something went.
    ///
    /// Case: a program that mounted a view on each screen disconnects from
    /// the control socket, so the host names every instance it registered
    /// in one removal.
    #[test]
    fn a_host_removal_reaches_both_screens() {
        let mut device = device();
        let primary = InstanceId(1);
        let alternate = InstanceId(2);
        assert!(mount(&mut device, primary));
        device.set_active_screen_for_test(ScreenKind::Alternate);
        assert!(mount(&mut device, alternate));

        assert!(device.remove_placements(&[primary, alternate]));
        assert_eq!(device.placement_count(), 0);
    }

    /// Asserts that the sweep reaches the inactive screen, so a placement
    /// whose anchor died there is still reclaimed.
    ///
    /// Case: `RIS` resets both screens while a webview is mounted on the
    /// one that is not currently shown.
    #[test]
    fn a_sweep_reaches_the_inactive_screen() {
        let mut device = device();
        device.set_active_screen_for_test(ScreenKind::Alternate);
        let id = InstanceId(1);
        assert!(mount(&mut device, id));
        assert_eq!(device.active_screen_mut().reset(), None);
        device.set_active_screen_for_test(ScreenKind::Primary);
        assert_eq!(device.evict_lost_anchors(), vec![id]);
    }

    /// Asserts that a reset leaves every placement unresolvable, so one
    /// terminal-wide sweep names all of them.
    ///
    /// Case: an application sends `RIS` while webviews are mounted on
    /// both the primary and the alternate screen.
    #[test]
    fn a_reset_leaves_every_placement_unresolvable() {
        let mut device = device();
        let primary = InstanceId(1);
        assert!(mount(&mut device, primary));
        device.set_active_screen_for_test(ScreenKind::Alternate);
        let alternate = InstanceId(2);
        assert!(mount(&mut device, alternate));

        let _ = device.reset();

        assert_eq!(device.evict_lost_anchors(), vec![primary, alternate]);
        assert_eq!(device.placement_count(), 0);
    }

    /// Asserts that a re-mount of a live id is accepted at the cap.
    ///
    /// Case: a program holding the terminal's last placement slot
    /// re-renders that same view.
    #[test]
    fn re_mounting_a_live_address_succeeds_at_the_cap() {
        let mut device = device();
        for index in 0..MAX_PLACEMENTS {
            assert!(mount(&mut device, InstanceId(index as u128)));
        }
        assert!(mount(&mut device, InstanceId(0)));
        assert_eq!(device.placement_count(), MAX_PLACEMENTS);
    }

    /// Asserts that a flip back to the primary screen tears down the
    /// placements the abandoned alternate screen owned and leaves the
    /// primary's alone.
    ///
    /// Case: a full-screen application that mounted a webview exits while
    /// the shell's own webview from before it is still mounted.
    #[test]
    fn a_flip_to_primary_tears_down_only_the_alternate_placements() {
        let mut device = device();
        let kept = InstanceId(1);
        assert!(mount(&mut device, kept));
        assert!(device.switch_screen(ScreenKind::Alternate).is_empty());
        let dropped = InstanceId(2);
        assert!(mount(&mut device, dropped));
        assert_eq!(device.switch_screen(ScreenKind::Primary), vec![dropped]);
        assert_eq!(device.placement_count(), 1);
        assert_eq!(device.active_screen_mut().take_placements(), vec![kept]);
    }

    /// Asserts that a set title reads back.
    ///
    /// Case: a shell prompt sets the window title and the host asks the
    /// device what it now says.
    #[test]
    fn a_set_title_reads_back() {
        let mut device = device();
        device.set_title(Some("hi".to_owned()));
        assert_eq!(device.title(), Some("hi"));
    }

    /// Asserts that a pushed title comes back off the stack.
    ///
    /// Case: a full-screen editor saves the title, sets its own, and
    /// restores the shell's on the way out.
    #[test]
    fn a_pushed_title_comes_back() {
        let mut device = device();
        device.set_title(Some("shell".to_owned()));
        device.push_title();
        device.set_title(Some("editor".to_owned()));
        assert_eq!(device.pop_title(), Some(Some("shell".to_owned())));
    }

    /// Asserts that popping an empty stack reports that it was empty.
    ///
    /// Case: a program restores a title it never saved.
    #[test]
    fn popping_an_empty_stack_reports_it() {
        let mut device = device();
        assert_eq!(device.pop_title(), None);
    }

    /// Asserts that a title pushed before any title was set comes back
    /// as the absence of one.
    ///
    /// Case: a program saves the title at startup, before the shell has
    /// set any, then restores it.
    #[test]
    fn pushing_before_any_title_pops_an_absence() {
        let mut device = device();
        device.push_title();
        device.set_title(Some("editor".to_owned()));
        assert_eq!(device.pop_title(), Some(None));
    }

    /// Asserts that a full stack drops its oldest entry rather than
    /// refusing the newest, and holds exactly its cap.
    ///
    /// Case: a runaway program pushes titles in a loop.
    #[test]
    fn a_full_stack_drops_its_oldest_entry() {
        let mut device = device();
        for n in 0..=MAX_TITLE_DEPTH {
            device.set_title(Some(n.to_string()));
            device.push_title();
        }
        let popped: Vec<Option<String>> = from_fn(|| device.pop_title()).collect();
        let expected: Vec<Option<String>> = (1..=MAX_TITLE_DEPTH)
            .rev()
            .map(|n| Some(n.to_string()))
            .collect();
        assert_eq!(popped, expected);
    }

    /// Asserts that a reset clears the title and its stack.
    ///
    /// Case: the shell sends `RIS` after a program left both a title
    /// and a saved one behind.
    #[test]
    fn a_reset_clears_the_title_and_its_stack() {
        let mut device = device();
        device.set_title(Some("shell".to_owned()));
        device.push_title();
        let _ = device.reset();
        assert_eq!(device.title(), None);
        assert_eq!(device.pop_title(), None);
    }

    /// Fills the active screen's first row to its last column, arming
    /// the deferred wrap.
    fn arm_deferred_wrap(device: &mut DeviceState) {
        for c in ['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h'] {
            device.print(c).expect("a printable glyph");
        }
    }

    /// The glyph at `column` of the active screen's `line`th visible row.
    fn glyph_at(device: &DeviceState, line: u16, column: u16) -> char {
        device.active_screen().viewport_row(ViewportLine(line))[column].c
    }

    /// Asserts that resetting autowrap disarms the deferred wrap on the
    /// hidden screen as well as the shown one, so a later set cannot
    /// cash in a latch armed before the reset.
    ///
    /// Case: a full-screen application fills the last column of the
    /// primary screen, enters the alternate screen, and turns autowrap
    /// off and on again there.
    #[test]
    fn resetting_autowrap_disarms_the_deferred_wrap_on_both_screens() {
        let mut device = device();
        arm_deferred_wrap(&mut device);
        device.set_active_screen_for_test(ScreenKind::Alternate);
        arm_deferred_wrap(&mut device);

        device.set_auto_wrap(AutoWrap::Disabled);
        device.set_auto_wrap(AutoWrap::Enabled);

        device.print('z').expect("a printable glyph");
        assert_eq!(glyph_at(&device, 0, 7), 'z');
        assert_eq!(glyph_at(&device, 1, 0), ' ');

        device.set_active_screen_for_test(ScreenKind::Primary);
        device.print('z').expect("a printable glyph");
        assert_eq!(glyph_at(&device, 0, 7), 'z');
        assert_eq!(glyph_at(&device, 1, 0), ' ');
    }

    /// Asserts that resetting autowrap leaves the saved cursor's
    /// deferred wrap alone rather than clearing it, so `DECRC` puts back
    /// the state `DECSC` captured.
    ///
    /// Case: an application fills a row, saves the cursor, turns
    /// autowrap off and on again, and restores the cursor.
    #[test]
    fn resetting_autowrap_leaves_the_saved_deferred_wrap_alone() {
        let mut device = device();
        arm_deferred_wrap(&mut device);
        device.active_screen_mut().save_checkpoint();

        device.set_auto_wrap(AutoWrap::Disabled);
        device.set_auto_wrap(AutoWrap::Enabled);
        device.active_screen_mut().restore_checkpoint();

        device.print('z').expect("a printable glyph");
        assert_eq!(glyph_at(&device, 1, 0), 'z');
    }

    /// Asserts that setting autowrap leaves an armed deferred wrap alone
    /// rather than disarming it, so the next character still wraps.
    ///
    /// Case: an application fills a row and re-sends `DECSET 7` while
    /// autowrap is already on.
    #[test]
    fn setting_autowrap_leaves_an_armed_deferred_wrap_alone() {
        let mut device = device();
        arm_deferred_wrap(&mut device);

        device.set_auto_wrap(AutoWrap::Enabled);

        device.print('z').expect("a printable glyph");
        assert_eq!(glyph_at(&device, 1, 0), 'z');
    }

    /// Asserts that a reset returns a recolored slot to its xterm
    /// default.
    ///
    /// Case: the user runs `reset` after a theme script recolored ANSI
    /// red.
    #[test]
    fn a_reset_restores_the_palette() {
        let mut device = device();
        device.set_indexed_color(1, Rgb { r: 1, g: 2, b: 3 });
        let _ = device.reset();
        assert_eq!(device.palette().indexed[1], Palette::default().indexed[1]);
    }

    /// Asserts that a reset reports a full repaint when it restores a
    /// recolored slot, even with nothing printed on either screen.
    ///
    /// Case: a theme script recolors the palette in a fresh terminal,
    /// and the user runs `reset` before anything is printed.
    #[test]
    fn a_reset_that_restores_the_palette_reports_a_full_repaint() {
        let mut device = device();
        device.set_indexed_color(1, Rgb { r: 1, g: 2, b: 3 });
        assert_eq!(device.reset(), Some(DamageSpan::Full));
    }

    /// Asserts that a soft reset returns each mode it names to its
    /// power-up value, and that autowrap returns to enabled rather than
    /// to the `No autowrap` of vt510.pdf p.277 Table 5-9.
    ///
    /// Case: a full-screen program leaves the caret hidden, insert mode
    /// on, autowrap off and the keypad in application mode, and the
    /// shell resets the terminal after it exits.
    #[test]
    fn a_soft_reset_returns_the_named_modes_to_their_defaults() {
        let mut device = device();
        let modes = device.modes_mut();
        modes.text_cursor.enable = TextCursorEnable::Hidden;
        modes.insert_replace = InsertReplaceMode::Insert;
        modes.app_cursor = true;
        modes.keypad_mode = KeypadMode::Application;
        device.set_auto_wrap(AutoWrap::Disabled);

        let _ = device.soft_reset();

        assert_eq!(device.modes().text_cursor.enable, TextCursorEnable::Shown);
        assert_eq!(device.modes().insert_replace, InsertReplaceMode::Replace);
        assert!(!device.modes().app_cursor);
        assert_eq!(device.modes().keypad_mode, KeypadMode::Numeric);
        assert_eq!(device.modes().auto_wrap, AutoWrap::Enabled);
    }

    /// Asserts that a soft reset leaves the modes and the title it does
    /// not name alone, unlike a hard reset.
    ///
    /// Case: a full-screen program with SGR mouse reporting, alternate
    /// scroll turned off, bracketed paste, focus reporting and a window
    /// title of its own issues a soft reset as part of its own start-up.
    #[test]
    fn a_soft_reset_leaves_the_state_it_does_not_name_alone() {
        let mut device = device();
        let modes = device.modes_mut();
        modes.mouse_tracking = MouseTracking::Clicks;
        modes.mouse_encoding = MouseEncoding::Sgr;
        modes.alternate_scroll = AlternateScroll::Disabled;
        modes.bracketed_paste = true;
        modes.focus_in_out = true;
        device.set_title(Some("build".to_string()));

        let _ = device.soft_reset();

        assert_eq!(device.modes().mouse_tracking, MouseTracking::Clicks);
        assert_eq!(device.modes().mouse_encoding, MouseEncoding::Sgr);
        assert_eq!(device.modes().alternate_scroll, AlternateScroll::Disabled);
        assert!(device.modes().bracketed_paste);
        assert!(device.modes().focus_in_out);
        assert_eq!(device.title(), Some("build"));
    }

    /// Asserts that a soft reset leaves the palette's default
    /// foreground and background alone while it returns the indexed
    /// slots.
    ///
    /// Case: the user's configured foreground and background are in
    /// force when a colour-scheme script recolours an indexed slot and
    /// the shell runs `tput init` afterwards.
    #[test]
    fn a_soft_reset_leaves_the_default_foreground_and_background_alone() {
        let mut device = device();
        let foreground = Rgb { r: 9, g: 8, b: 7 };
        let background = Rgb { r: 6, g: 5, b: 4 };
        device.palette.foreground = foreground;
        device.palette.background = background;
        assert!(device.set_indexed_color(1, Rgb { r: 1, g: 2, b: 3 }));

        let _ = device.soft_reset();

        assert_eq!(device.palette().foreground, foreground);
        assert_eq!(device.palette().background, background);
    }

    /// Asserts that a soft reset leaves the cursor color alone.
    ///
    /// Case: an editor's cursor colour is in force when the shell runs
    /// `tput init`.
    #[test]
    fn a_soft_reset_leaves_the_cursor_color_alone() {
        let mut device = device();
        let cursor = Rgb { r: 9, g: 8, b: 7 };
        assert!(device.set_cursor_color(cursor));

        let _ = device.soft_reset();

        assert_eq!(device.palette().cursor, Some(cursor));
    }

    /// Asserts that a reset returns the cursor color to unset.
    ///
    /// Case: the user runs `reset` after an editor crashed with its
    /// cursor colour still in force.
    #[test]
    fn a_reset_clears_the_cursor_color() {
        let mut device = device();
        assert!(device.set_cursor_color(Rgb { r: 9, g: 8, b: 7 }));

        let _ = device.reset();

        assert_eq!(device.palette().cursor, None);
    }

    /// Asserts that resetting the cursor color reports a change only
    /// when a color was held.
    ///
    /// Case: a program sends `OSC 112` on exit without ever having set
    /// a cursor colour.
    #[test]
    fn resetting_an_unset_cursor_color_reports_no_change() {
        let mut device = device();
        assert!(!device.reset_cursor_color());
        assert!(device.set_cursor_color(Rgb { r: 9, g: 8, b: 7 }));
        assert!(device.reset_cursor_color());
    }

    /// Asserts that a soft reset keeps the screen the device was
    /// showing, unlike a hard reset.
    ///
    /// Case: a full-screen editor issues a soft reset after it has
    /// already taken the alternate screen.
    #[test]
    fn a_soft_reset_keeps_the_screen_on_show() {
        let mut device = device();
        device.set_active_screen_for_test(ScreenKind::Alternate);

        let _ = device.soft_reset();

        assert_eq!(device.modes().active_screen, ScreenKind::Alternate);
    }

    /// Asserts that a soft reset returns every indexed palette slot to
    /// its built-in default and reports the repaint that owes.
    ///
    /// Case: a colour-scheme script recolours the palette with `OSC 4`
    /// and the shell resets the terminal afterwards.
    #[test]
    fn a_soft_reset_returns_the_indexed_palette_to_its_default() {
        let mut device = device();
        let default_first = device.palette().indexed[1];
        assert!(device.set_indexed_color(1, Rgb { r: 1, g: 2, b: 3 }));

        let damage = device.soft_reset();

        assert_eq!(device.palette().indexed[1], default_first);
        assert_eq!(damage, Some(DamageSpan::Full));
    }

    /// Asserts that a soft reset over an untouched palette reports no
    /// repaint.
    ///
    /// Case: the shell runs `tput init` on a terminal no program has
    /// recoloured.
    #[test]
    fn a_soft_reset_over_an_untouched_palette_reports_no_repaint() {
        let mut device = device();

        assert_eq!(device.soft_reset(), None);
    }

    /// Asserts that a soft reset reaches the screen on show and leaves
    /// the hidden screen's pen and character set mapping as they are.
    ///
    /// Case: a full-screen program takes the alternate screen and soft
    /// resets it, and the shell goes on printing on the primary screen
    /// after the program exits.
    #[test]
    fn a_soft_reset_reaches_only_the_screen_on_show() {
        let mut device = device();
        for kind in [ScreenKind::Primary, ScreenKind::Alternate] {
            device.set_active_screen_for_test(kind);
            device.active_screen_mut().pen_mut().fg = Color::Indexed(1);
            device
                .active_screen_mut()
                .designate_character_set(GCode::G0, CharacterSet::DecSpecialGraphics);
        }

        let _ = device.soft_reset();

        device.print('q').expect("a printable glyph");
        let shown = &device.active_screen().viewport_row(ViewportLine(0))[0];
        assert_eq!(shown.c, 'q');
        assert_eq!(shown.fg, Color::DefaultForeground);

        device.set_active_screen_for_test(ScreenKind::Primary);
        device.print('q').expect("a printable glyph");
        let hidden = &device.active_screen().viewport_row(ViewportLine(0))[0];
        assert_eq!(hidden.c, '─');
        assert_eq!(hidden.fg, Color::Indexed(1));
    }
}
