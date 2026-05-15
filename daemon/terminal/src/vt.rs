//! Server-side VT emulator integration via alacritty_terminal.

pub(crate) mod bridge;
pub(crate) mod coalescer;
pub(crate) mod frame;
pub(crate) mod frame_builder;
pub(crate) mod frame_ring;
pub(crate) mod hyperlink;
pub(crate) mod listener;
pub(crate) mod mode_diff;

pub use coalescer::{Coalescer, DamageVerdict};
pub use frame::{
    Color, Cursor, CursorShape, DirtyRow, FrameDelta, FrameSnapshot, Hyperlink, ModeFrame,
    ModeKind, RenderFrame, Row, Run, SnapshotReason, encode,
};
pub use frame_builder::DirtyRows;
pub use frame_ring::{EncodedDelta, FrameRing, WireMessage};
pub use hyperlink::{AlacrittyHyperlinkId, HyperlinkInterner, HyperlinkUri, HyperlinkWireId};
pub use listener::{ControlFrame, DropCounter, ReplyFrame, TermListener};
pub use mode_diff::{ModeChange, diff_mode};
