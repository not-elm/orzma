//! Per-mode processing. `AppMode` selects which mode is active; each mode
//! declares its systems inside a dedicated submodule.

use bevy::prelude::*;

/// Application mode. `Default` is the default (single PTY, no tmux).
/// `Tmux` activates the tmux multiplexer backend.
#[derive(States, Debug, Clone, PartialEq, Eq, Hash, Default)]
pub(crate) enum AppMode {
    /// Single PTY terminal, Alacritty VT emulation, no tmux.
    #[default]
    Default,
    /// tmux backend, multiplexer layout.
    Tmux,
}

pub(crate) mod default;
pub(crate) mod tmux;
