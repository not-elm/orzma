//! Server-side VT emulator integration via alacritty_terminal.
//!
//! このモジュールは PTY 出力を alacritty_terminal::Term で解釈し、
//! 構造化された描画 frame に変換する経路を提供する。Phase 1 では
//! Term を内部更新するだけで wire には何も emit しない。

pub(crate) mod bridge;
pub(crate) mod frame;
pub(crate) mod frame_builder;
pub(crate) mod frame_ring;
pub(crate) mod hyperlink;
pub(crate) mod listener;
pub(crate) mod mode_diff;

pub use frame::{
    Color, Cursor, CursorShape, DirtyRow, FrameDelta, FrameSnapshot, Hyperlink, ModeFrame,
    ModeKind, RenderFrame, Row, Run, SnapshotReason, encode,
};
pub use frame_builder::DirtyRows;
pub use frame_ring::{EncodedDelta, FrameRing, WireMessage};
pub use hyperlink::HyperlinkInterner;
pub use listener::{ControlFrame, DropCounter, ReplyFrame, TermListener};
pub use mode_diff::{ModeChange, diff_mode};
