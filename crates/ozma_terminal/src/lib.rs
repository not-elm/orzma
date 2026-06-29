//! Ozma standalone VT terminal component: Bevy plugin and shared types.

mod action;
mod clipboard;
mod exit;
mod hyperlink;
mod layout;
mod mouse;
mod spawn;

use crate::action::OzmaActionPlugin;
use crate::mouse::OzmaMousePlugin;
use crate::spawn::on_add_inject_render;
use crate::{exit::ExitPlugin, layout::LayoutPlugin};
pub use action::PasteAction;
use bevy::prelude::*;
pub use clipboard::{Clipboard, build_paste_bytes};
pub use mouse::{
    MouseEffect, TerminalForwardInput, TerminalMouseEffects, TerminalMouseWrite, TerminalOpenUri,
    TerminalSelectionClear, TerminalSelectionCopy, TerminalSelectionStart, TerminalSelectionUpdate,
    TerminalViewportScroll,
};
pub use spawn::{
    OzmaSpawnOptions, OzmaTerminal, OzmaTerminalBundle, OzmaTerminalConfig, cells_for,
    resolve_shell,
};

/// Bevy plugin that registers the Ozma VT terminal subsystems.
///
/// Adds `ExitPlugin` (fires `AppExit` on shell exit) and `LayoutPlugin`
/// (window-fill resize). Does not call `insert_state` — consumers must
/// manage `AppMode` and spawn `OzmaTerminal` entities independently.
pub struct OzmaTerminalPlugin {
    /// Shell override. `None` defers to `$SHELL` at spawn time.
    pub config_shell: Option<String>,
}

impl Plugin for OzmaTerminalPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(OzmaTerminalConfig {
            shell: self.config_shell.clone(),
        })
        .add_plugins((ExitPlugin, LayoutPlugin, OzmaActionPlugin, OzmaMousePlugin))
        .add_observer(on_add_inject_render);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::asset::AssetPlugin;
    use ozma_tty_renderer::material::TerminalUiMaterial;

    #[test]
    fn plugin_registers_config_resource() {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            AssetPlugin::default(),
            OzmaTerminalPlugin {
                config_shell: Some("/bin/fish".into()),
            },
        ));
        app.world_mut()
            .init_resource::<bevy::asset::Assets<TerminalUiMaterial>>();
        app.update();
        assert_eq!(
            app.world()
                .resource::<OzmaTerminalConfig>()
                .shell
                .as_deref(),
            Some("/bin/fish"),
        );
    }
}
