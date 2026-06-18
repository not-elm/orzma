//! Ozma single-terminal mode: owns `AppMode` State and the `OzmaModePlugin`.

mod exit;
mod layout;
mod spawn;

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

/// Internal resource storing the constructor-injected shell override.
/// `None` means "fall back to `$SHELL` at spawn time".
#[derive(Resource)]
pub(crate) struct OzmaModeConfig {
    pub(crate) shell: Option<String>,
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
            .init_resource::<layout::OzmaLastSize>()
            .add_message::<bevy::window::WindowResized>()
            .insert_resource(OzmaModeConfig {
                shell: self.config_shell.clone(),
            })
            .add_observer(exit::on_child_exit)
            .add_systems(OnEnter(AppMode::Ozma), spawn::spawn_terminal)
            .add_systems(
                Update,
                layout::resize_to_window
                    .run_if(in_state(AppMode::Ozma))
                    .run_if(
                        resource_exists_and_changed::<layout::OzmaLastSize>
                            .or(resource_exists_and_changed::<
                                ozma_tty_renderer::TerminalCellMetricsResource,
                            >)
                            .or(on_message::<bevy::window::WindowResized>),
                    ),
            )
            .add_systems(OnExit(AppMode::Ozma), spawn::despawn_terminal);
    }
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
