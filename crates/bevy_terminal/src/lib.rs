//! Bevy-native terminal: PTY ownership, alacritty VT emulation, and
//! coalesced `FrameSnapshot` / `FrameDelta` emission against the
//! `bevy_terminal_renderer` schema.

mod bundle;
mod buttons;
mod coalescer;
mod events;
mod handle;
mod input_codec;
mod mouse_encode;
mod osc7;
mod palette;
mod plugin;
mod pty;
mod title;
mod vt;
mod wheel;

pub use alacritty_terminal::index::{Column, Line, Point, Side};
pub use alacritty_terminal::selection::SelectionType;
pub use alacritty_terminal::term::TermMode;
pub use alacritty_terminal::vi_mode::ViMotion;
pub use bundle::{SpawnOptions, TerminalBundle};
pub use buttons::{ButtonAction, ButtonConfig, ButtonEvent, ButtonEventKind, MouseButtonKind};
pub use coalescer::Coalescer;
pub use events::{
    TerminalBell, TerminalChildExit, TerminalClipboardStore, TerminalCurrentDir, TerminalKey,
    TerminalKeyInput, TerminalModeChanged, TerminalModifiers, TerminalTitleChanged,
};
pub use handle::{TerminalHandle, ViIndicatorSnapshot};
pub use mouse_encode::ProtocolModifiers;
pub use plugin::TerminalHandlePlugin;
pub use pty::PtyHandle;
pub use title::TerminalTitle;
pub use wheel::{CellCoord, WheelAction, WheelConfig, WheelDir, WheelModifiers};
