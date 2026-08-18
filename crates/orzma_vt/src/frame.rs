//! Frame emission: turning one staged damage into one [`Frame`].
//!
//! [`FrameEmitter`] holds only what persists across emits without
//! belonging to the terminal — the hyperlink interner — and reads
//! terminal and placement state
//! immutably, so a frame describes a single instant. Deciding *whether*
//! a frame is a snapshot is not its job: it builds whatever the damage
//! it receives classifies as.

use crate::damage::Damage;
use crate::hyperlink::HyperlinkInterner;
use crate::schema::{Frame, FrameDelta, FrameSnapshot, Row, Run, ViewportLine};
use crate::screen::Screen;

/// Emission state: the hyperlink interner the emitted runs share.
#[expect(
    dead_code,
    reason = "OrzmaVt reaches the emitter once it gains its fields"
)]
pub(crate) struct FrameEmitter {
    hyperlinks: HyperlinkInterner,
}

#[expect(
    dead_code,
    reason = "OrzmaVt reaches the emitter once it gains its fields"
)]
impl FrameEmitter {
    /// Builds an emitter with an empty hyperlink interner.
    pub fn new() -> Self {
        todo!()
    }

    /// Builds the frame for `damage`.
    ///
    /// [`Damage::Full`] yields a [`Frame::Snapshot`] and row damage a
    /// [`Frame::Delta`], including an empty one whose metadata is still
    /// current.
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
    fn row(&mut self, _line: ViewportLine, _screen: &Screen) -> Row<Run> {
        todo!()
    }
}
