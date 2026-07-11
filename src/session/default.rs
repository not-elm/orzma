//! Default-mode shell lifecycle: PTY spawn config, window-fill layout, and
//! exit-on-shell-quit.

pub(crate) mod spawn;

mod exit;
mod layout;

use bevy::prelude::*;

/// Bevy plugin for the Default-mode shell lifecycle (spawn / layout / exit).
pub(crate) struct DefaultSessionPlugin;

impl Plugin for DefaultSessionPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            spawn::DefaultSpawnPlugin,
            exit::DefaultExitPlugin,
            layout::DefaultLayoutPlugin,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_mode::AppMode;
    use crate::configs::OrzmaConfigsResource;
    use bevy::state::app::StatesPlugin;
    use spawn::OrzmaTerminalConfig;

    #[test]
    fn shell_synced_from_resolved_config_at_startup() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin));
        app.insert_state(AppMode::Default);
        let mut cfg = orzma_configs::OrzmaConfigs::default();
        cfg.orzma.shell = Some("/usr/bin/fish".into());
        app.insert_resource(OrzmaConfigsResource(cfg));
        app.add_plugins(DefaultSessionPlugin);
        app.update();
        assert_eq!(
            app.world()
                .resource::<OrzmaTerminalConfig>()
                .shell
                .as_deref(),
            Some("/usr/bin/fish"),
            "DefaultSessionPlugin must sync shell from OrzmaConfigsResource into OrzmaTerminalConfig at Startup",
        );
    }
}
