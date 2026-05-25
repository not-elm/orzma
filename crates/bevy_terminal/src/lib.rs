//! Bevy-native terminal: PTY ownership, alacritty VT emulation, and
//! coalesced `FrameSnapshot` / `FrameDelta` emission against the
//! `bevy_terminal_renderer` schema.

mod bundle;
mod coalescer;
mod events;
mod handle;
mod input_codec;
mod palette;
mod plugin;
mod pty;
mod title;
mod vt;

pub use alacritty_terminal::selection::SelectionType;
pub use alacritty_terminal::vi_mode::ViMotion;
pub use bundle::{SpawnOptions, TerminalBundle};
pub use coalescer::Coalescer;
pub use events::{
    TerminalBell, TerminalChildExit, TerminalClipboardStore, TerminalKey, TerminalKeyInput,
    TerminalModeChanged, TerminalModifiers, TerminalTitleChanged,
};
pub use handle::{TerminalHandle, ViIndicatorSnapshot};
pub use plugin::TerminalHandlePlugin;
pub use pty::PtyHandle;
pub use title::TerminalTitle;
