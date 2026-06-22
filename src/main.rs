//! ozmux Bevy GUI entry point.

mod app_mode;
mod bootstrap;
mod cef_profile;
mod configs;
mod default_input;
mod font;
mod input;
mod picker;
mod system_set;
mod theme;
mod tmux;
mod ui;
mod window_title;

use crate::app_mode::{AppMode, DefaultModePlugin};
use crate::cef_profile::CefProfileDir;
use crate::input::hyperlink::HyperlinkInputPlugin;
use crate::window_title::WindowTitlePlugin;
use bevy::prelude::*;
use bootstrap::OzmuxBootstrapPlugin;
use configs::OzmuxConfigsPlugin;
use default_input::DefaultHostInputPlugin;
use font::FontBridgePlugin;
use input::OzmuxShortcutPlugin;
use input::ime::ImePlugin;
use input::option_as_alt::OptionAsAltPlugin;
use ozma_terminal::OzmaTerminalPlugin;
use ozma_tty_engine::TerminalHandlePlugin;
use ozma_tty_renderer::TerminalRendererPlugin;
use ozma_webview::{OzmaWebviewPlugin, cef_plugin};
use ozmux_configs::StartupMode;
use ozmux_webview_host::WebviewAssetRegistry;
use picker::OzmuxPickerPlugin;
use tmux::OzmuxTmuxPlugin;
use ui::ime_overlay::ImeOverlayPlugin;
use ui::{
    OzmuxUiPlugin, confirm_prompt::ConfirmPromptPlugin, copy_mode::CopyModePlugin,
    copy_mode_indicator::CopyModeIndicatorPlugin, copy_search::CopyPromptPlugin,
    rename_prompt::RenamePromptPlugin,
};

/// The primary window descriptor.
///
/// `ime_enabled` starts `false` deliberately: bevy_winit applies the IME state
/// to the OS window only on a live `false -> true` change of `Window::ime_enabled`
/// (`bevy_winit-0.18.1/src/system.rs:503-504`) and never at window creation, so
/// starting `true` would leave the OS IME un-armed. `ime_policy_system` flips it
/// to `true` on the first focused-surface tick, producing the arming transition.
fn primary_window() -> Window {
    Window {
        title: "ozmux".to_string(),
        ime_enabled: false,
        ..default()
    }
}

fn main() {
    let pre_configs = ozmux_configs::OzmuxConfigs::load().unwrap_or_default();
    // NOTE: start in AppMode::Tmux as a boot-dispatch state; dispatch_startup_mode
    // (OnEnter(Tmux), gated to run once) routes to the real mode. Routing to Default
    // via a queued NextState — rather than booting straight into Default — defers
    // OnEnter(AppMode::Default) (spawn_terminal) to a post-Startup StateTransition, so
    // Startup deferred commands (e.g. init_atlas_image inserting AtlasImage) flush first.
    let initial_mode = match pre_configs.startup_mode {
        StartupMode::Default | StartupMode::Tmux | StartupMode::TmuxAutoAttach => AppMode::Tmux,
    };
    let ozma_registry = WebviewAssetRegistry::default();
    let cef_profile = CefProfileDir::acquire().expect("create per-process CEF profile directory");
    App::new()
        .add_plugins((
            DefaultPlugins.set(WindowPlugin {
                primary_window: Some(primary_window()),
                ..default()
            }),
            cef_plugin(ozma_registry.clone(), cef_profile.path()),
        ))
        .insert_state(initial_mode)
        .add_plugins((
            OzmaTerminalPlugin {
                config_shell: pre_configs.ozma.shell.clone(),
            },
            DefaultModePlugin,
            TerminalHandlePlugin,
            TerminalRendererPlugin,
            OzmuxTmuxPlugin,
            OzmuxPickerPlugin,
            OzmuxConfigsPlugin,
            FontBridgePlugin,
            OzmuxBootstrapPlugin,
            OzmuxShortcutPlugin,
            OzmuxUiPlugin,
            OzmaWebviewPlugin {
                osc_enabled: pre_configs.osc_webview.enabled,
                ozma_assets: ozma_registry,
            },
            CopyModePlugin,
            CopyModeIndicatorPlugin,
        ))
        .add_plugins(CopyPromptPlugin)
        .add_plugins(ConfirmPromptPlugin)
        .add_plugins(RenamePromptPlugin)
        .add_plugins(WindowTitlePlugin)
        .add_plugins((
            HyperlinkInputPlugin,
            ImePlugin,
            ImeOverlayPlugin,
            OptionAsAltPlugin,
            DefaultHostInputPlugin,
        ))
        .run();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_window_starts_with_ime_disabled() {
        // NOTE: bevy_winit never applies `ime_enabled` at window creation; it
        // calls `set_ime_allowed` only on a live `false -> true` change. Starting
        // `true` means that transition never fires and the OS IME never arms.
        assert!(!primary_window().ime_enabled);
    }
}
