//! Ozma single-terminal mode: owns `AppMode` State and the `OzmaModePlugin`.

mod exit;
mod layout;
mod spawn;

pub use spawn::OzmaTerminal;
use crate::{exit::ExitPlugin, layout::LayoutPlugin, spawn::SpawnPlugin};
use bevy::prelude::*;

/// Application mode. `Ozma` is the default (single PTY, no tmux).
/// `Ozmux` activates the tmux multiplexer backend.
///
/// Owned here and re-exported so other crates can depend on this crate
/// for the shared state type rather than duplicating it.
#[derive(States, Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum AppMode {
    /// Single PTY terminal, Alacritty VT emulation, no tmux.
    #[default]
    Ozma,
    /// tmux backend, multiplexer layout.
    Ozmux,
}

/// Bevy plugin implementing Ozma mode: spawns a single PTY terminal on
/// `OnEnter(AppMode::Ozma)`, resizes it to fill the window, and sends
/// `AppExit` when the shell process exits.
pub struct OzmaModePlugin {
    config_shell: Option<String>,
}

impl OzmaModePlugin {
    /// Constructs the plugin with the shell override from config.
    ///
    /// Pass `OzmuxConfigs.ozma.shell` here; the plugin resolves
    /// `$SHELL` and `/bin/sh` fallbacks at spawn time.
    pub fn new(config_shell: Option<String>) -> Self {
        Self { config_shell }
    }
}

impl Plugin for OzmaModePlugin {
    fn build(&self, app: &mut App) {
        app.init_state::<AppMode>()
            .insert_resource(OzmaModeConfig {
                shell: self.config_shell.clone(),
            })
            .add_plugins((ExitPlugin, LayoutPlugin, SpawnPlugin));
    }
}

/// Internal resource storing the constructor-injected shell override.
/// `None` means "fall back to `$SHELL` at spawn time".
#[derive(Resource)]
pub(crate) struct OzmaModeConfig {
    pub(crate) shell: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::asset::AssetPlugin;
    use ozma_tty_renderer::material::TerminalUiMaterial;

    #[test]
    fn plugin_registers_state_and_defaults_to_ozma() {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            AssetPlugin::default(),
            bevy::state::app::StatesPlugin,
            OzmaModePlugin::new(None),
        ));
        app.world_mut()
            .init_resource::<bevy::asset::Assets<TerminalUiMaterial>>();
        app.update();
        assert_eq!(
            app.world().resource::<State<AppMode>>().get(),
            &AppMode::Ozma,
        );
    }
}
