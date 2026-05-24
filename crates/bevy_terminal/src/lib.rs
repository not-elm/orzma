//! Bevy-native terminal: PTY ownership, alacritty VT emulation, and
//! coalesced `FrameSnapshot` / `FrameDelta` emission against the
//! `bevy_terminal_render` schema.

mod bundle;
mod coalescer;
mod events;
mod handle;
mod palette;
mod plugin;
mod pty;
mod title;
mod vt;

pub use bundle::{SpawnOptions, TerminalBundle};
pub use coalescer::Coalescer;
pub use events::{
    TerminalBell, TerminalChildExit, TerminalClipboardStore, TerminalModeChanged,
    TerminalTitleChanged,
};
pub use handle::TerminalHandle;
pub use plugin::TerminalHandlePlugin;
pub use pty::PtyHandle;
pub use title::TerminalTitle;
