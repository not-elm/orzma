//! Bevy-native terminal: PTY ownership, alacritty VT emulation, and
//! coalesced `TerminalSnapshot` / `TerminalDelta` emission (wrapping the
//! pure `ozmux_vt::frame` payloads) against the
//! `bevy_terminal_renderer` schema.

mod bundle;
mod buttons;
mod events;
mod handle;
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
    TerminalBell, TerminalChildExit, TerminalClipboardStore, TerminalCurrentDir, TerminalKeyInput,
    TerminalModeChanged, TerminalTitleChanged,
};
pub use handle::TerminalHandle;
pub use ozmux_vt::input::{TerminalKey, TerminalModifiers, encode_key};
pub use ozmux_vt::mouse::ProtocolModifiers;
pub use ozmux_vt::vt::ViIndicatorSnapshot;
pub use ozmux_vt::vt::mode_diff::modes_from_names;
pub use plugin::TerminalHandlePlugin;
pub use pty::PtyHandle;
pub use title::{TerminalTitle, sanitize_title};
pub use wheel::{CellCoord, WheelAction, WheelConfig, WheelDir, WheelModifiers};
