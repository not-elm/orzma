//! Server-side VT emulator integration via alacritty_terminal.
//!
//! このモジュールは PTY 出力を alacritty_terminal::Term で解釈し、
//! 構造化された描画 frame に変換する経路を提供する。Phase 1 では
//! Term を内部更新するだけで wire には何も emit しない。

pub mod bridge;
pub mod frame;
pub mod frame_ring;
pub mod listener;
pub mod mode_diff;
