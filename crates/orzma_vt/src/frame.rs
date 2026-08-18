//! Frame emission: turning one staged damage into one [`Frame`].
//!
//! [`FrameEmitter`] holds only what persists across emits without
//! belonging to the terminal — the wrapping sequence number and the
//! hyperlink interner — and reads terminal and placement state
//! immutably, so a frame describes a single instant. Deciding *whether*
//! a frame is a snapshot is not its job: it builds whatever the damage
//! it receives classifies as.

use crate::damage::Damage;
use crate::hyperlink::HyperlinkInterner;
use crate::schema::{Frame, FrameDelta, FrameSnapshot, Row, ViewportLine};
use crate::screen::Screen;

/// Emission state: the sequence counter and the hyperlink interner.
#[expect(
    dead_code,
    reason = "OrzmaVt reaches the emitter once it gains its fields"
)]
pub(crate) struct FrameEmitter {
    next_seq: u32,
    hyperlinks: HyperlinkInterner,
}

#[expect(
    dead_code,
    reason = "OrzmaVt reaches the emitter once it gains its fields"
)]
impl FrameEmitter {
    /// Builds an emitter that stamps sequence zero on its first frame.
    pub fn new() -> Self {
        todo!()
    }

    /// Builds the frame for `damage`, advancing the sequence.
    ///
    /// [`Damage::Full`] yields a [`Frame::Snapshot`] and row damage a
    /// [`Frame::Delta`], including an empty one whose metadata is still
    /// current.
    // TODO: Accept the terminal state and the placement store once they
    // exist; a single screen stands in for both today, which is why
    // modes, palette, and placements are still stubbed.
    pub fn emit(&mut self, _damage: Damage, _screen: &Screen) -> Frame {
        todo!()
    }

    /// Builds a full repaint of the visible viewport.
    fn snapshot(&mut self, _screen: &Screen) -> FrameSnapshot {
        todo!()
    }

    /// Builds a differential update covering the damaged rows.
    fn delta(&mut self, _rows: &[u16], _screen: &Screen) -> FrameDelta {
        todo!()
    }

    /// Builds one viewport row as attribute runs, absorbing wide-char
    /// spacers.
    // TODO: Read cells through a crate-private projection on `Screen`.
    // `Grid`'s index resolves against the live tail, so a viewport
    // scrolled into history cannot be read through it.
    // TODO: Intern OSC 8 links here once `HyperlinkInterner` and the
    // frame schema agree on one `HyperlinkId`.
    fn row(&mut self, _line: ViewportLine, _screen: &Screen) -> Row {
        todo!()
    }

    /// Hands out the sequence for the frame under construction and
    /// advances the counter.
    ///
    /// # Invariants
    ///
    /// Call exactly once per emitted frame: the [`crate::Vt::frame`]
    /// contract pins the sequence to advance per frame and never on a
    /// `None` return.
    fn take_seq(&mut self) -> u32 {
        todo!()
    }
}
