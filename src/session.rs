//! Session lifecycle of the single-PTY shell: spawn config, window-fill
//! layout, and exit-on-shell-quit. One terminal per session today; the local
//! multiplexer will generalize this module.

pub(crate) mod spawn;

mod exit;
mod layout;

use bevy::prelude::*;

/// Bevy plugin for the shell session lifecycle (spawn / layout / exit).
pub(crate) struct SessionPlugin {
    /// Shell override forwarded to `spawn::SpawnPlugin`.
    pub shell: Option<String>,
}

impl Plugin for SessionPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            spawn::SpawnPlugin {
                shell: self.shell.clone(),
            },
            exit::ExitPlugin,
            layout::LayoutPlugin,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spawn::OrzmaTerminalConfig;

    #[test]
    fn config_shell_forwards_to_orzma_terminal_config() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(SessionPlugin {
            shell: Some("/bin/fish".into()),
        });
        assert_eq!(
            app.world()
                .resource::<OrzmaTerminalConfig>()
                .shell
                .as_deref(),
            Some("/bin/fish"),
            "SessionPlugin must forward shell through SpawnPlugin into OrzmaTerminalConfig",
        );
    }
}
