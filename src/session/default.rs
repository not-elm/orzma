//! Default-mode shell lifecycle: PTY spawn config, window-fill layout, and
//! exit-on-shell-quit.

pub(crate) mod spawn;

mod exit;
mod layout;

use bevy::prelude::*;

/// Bevy plugin for the Default-mode shell lifecycle (spawn / layout / exit).
pub(crate) struct DefaultSessionPlugin {
    /// Shell override forwarded to `spawn::DefaultSpawnPlugin`.
    pub shell: Option<String>,
}

impl Plugin for DefaultSessionPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            spawn::DefaultSpawnPlugin {
                shell: self.shell.clone(),
            },
            exit::DefaultExitPlugin,
            layout::DefaultLayoutPlugin,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_mode::AppMode;
    use bevy::state::app::StatesPlugin;
    use spawn::OzmaTerminalConfig;

    #[test]
    fn config_shell_forwards_to_ozma_terminal_config() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin));
        app.insert_state(AppMode::Default);
        app.add_plugins(DefaultSessionPlugin {
            shell: Some("/bin/fish".into()),
        });
        assert_eq!(
            app.world()
                .resource::<OzmaTerminalConfig>()
                .shell
                .as_deref(),
            Some("/bin/fish"),
            "DefaultSessionPlugin must forward shell through DefaultSpawnPlugin into OzmaTerminalConfig",
        );
    }
}
