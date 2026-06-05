//! Bevy-native terminal: PTY ownership, alacritty VT emulation, and
//! coalesced `TerminalSnapshot` / `TerminalDelta` emission (wrapping the
//! pure `ozmux_vt::frame` payloads) against the
//! `bevy_terminal_renderer` schema.

mod bundle;
mod buttons;
mod events;
mod handle;
mod input_codec;
mod mouse_encode;
mod plugin;
mod pty;
mod title;
mod wheel;

pub use alacritty_terminal::index::{Column, Line, Point, Side};
pub use alacritty_terminal::selection::SelectionType;
pub use alacritty_terminal::term::TermMode;
pub use alacritty_terminal::vi_mode::ViMotion;
pub use bundle::{SpawnOptions, TerminalBundle};
pub use buttons::{ButtonAction, ButtonConfig, ButtonEvent, ButtonEventKind, MouseButtonKind};
pub use events::{
    TerminalBell, TerminalChildExit, TerminalClipboardStore, TerminalCurrentDir, TerminalKey,
    TerminalKeyInput, TerminalModeChanged, TerminalModifiers, TerminalTitleChanged,
};
pub use handle::TerminalHandle;
pub use mouse_encode::ProtocolModifiers;
pub use ozmux_vt::vt::ViIndicatorSnapshot;
pub use plugin::TerminalHandlePlugin;
pub use pty::PtyHandle;
pub use title::{TerminalTitle, sanitize_title};
pub use wheel::{CellCoord, WheelAction, WheelConfig, WheelDir, WheelModifiers};
