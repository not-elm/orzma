//! Server-side VT emulator integration via alacritty_terminal.

pub(crate) mod bridge;
pub(crate) mod coalescer;
pub(crate) mod frame;
pub(crate) mod frame_builder;
pub(crate) mod frame_ring;
pub(crate) mod hyperlink;
pub(crate) mod listener;
pub(crate) mod mode_diff;
pub(crate) mod produced_at;
pub(crate) mod title;

pub use coalescer::Coalescer;
pub use frame::{
    Color, Cursor, CursorShape, DirtyRow, FrameDelta, FrameSnapshot, Hyperlink, ModeFrame,
    RenderFrame, Row, Run, SnapshotReason, encode,
};
pub use frame_ring::WireMessage;
pub use hyperlink::{HyperlinkUri, HyperlinkWireId};
