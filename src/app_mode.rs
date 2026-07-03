//! Application mode state and the tmux activity gate. `AppMode` selects which
//! mode is active; features gate their tmux systems on `TmuxActiveSet`.

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

/// SystemSet applied to every tmux Update system. Runs only in `AppMode::Tmux`.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct TmuxActiveSet;

/// Bevy plugin owning `AppMode` state initialization and the `TmuxActiveSet`
/// run gate.
///
/// The app boots into `AppMode::Default` (the `#[default]` variant); tmux is
/// entered only by adopting the user's own `tmux -CC`
/// (`ControlModeDetected` -> `NextState(Tmux)`), never at boot.
pub(crate) struct AppModePlugin;

impl Plugin for AppModePlugin {
    fn build(&self, app: &mut App) {
        app.init_state::<AppMode>()
            .configure_sets(Update, TmuxActiveSet.run_if(in_state(AppMode::Tmux)));
    }
}
