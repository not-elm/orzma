//! ozmux Bevy GUI entry point.

mod bootstrap;
mod cef_profile;
mod configs;
mod control_plane;
mod font;
mod input;
mod ozma;
mod ozma_input;
mod picker;
mod system_set;
mod theme;
mod tmux;
mod ui;
mod webview;
mod window_title;

use crate::cef_profile::CefProfileDir;
use crate::control_plane::OzmuxControlPlanePlugin;
use crate::input::hyperlink::HyperlinkInputPlugin;
use crate::ozma::{AppMode, OzmaModePlugin};
use crate::webview::OzmuxWebviewPlugin;
use crate::webview::render::cef_plugin;
use crate::window_title::WindowTitlePlugin;
use bevy::prelude::*;
use bootstrap::OzmuxBootstrapPlugin;
use configs::OzmuxConfigsPlugin;
use font::FontBridgePlugin;
use input::OzmuxShortcutPlugin;
use input::ime::ImePlugin;
use input::option_as_alt::OptionAsAltPlugin;
use ozma_input::OzmaHostInputPlugin;
use ozma_terminal::OzmaTerminalPlugin;
use ozma_tty_engine::TerminalHandlePlugin;
use ozma_tty_renderer::TerminalRendererPlugin;
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

fn main() {
    let pre_configs = ozmux_configs::OzmuxConfigs::load().unwrap_or_default();
    // NOTE: start in AppMode::Ozmux as a boot-dispatch state; dispatch_startup_mode
    // (OnEnter(Ozmux), gated to run once) routes to the real mode. Routing to Ozma
    // via a queued NextState — rather than booting straight into Ozma — defers
    // OnEnter(AppMode::Ozma) (spawn_terminal) to a post-Startup StateTransition, so
    // Startup deferred commands (e.g. init_atlas_image inserting AtlasImage) flush first.
    let initial_mode = match pre_configs.startup_mode {
        StartupMode::Ozma | StartupMode::Ozmux | StartupMode::AutoAttach => AppMode::Ozmux,
    };
    let dyn_registry = WebviewAssetRegistry::default();
    let cef_profile = CefProfileDir::acquire().expect("create per-process CEF profile directory");
    App::new()
        .add_plugins((
            DefaultPlugins.set(WindowPlugin {
                primary_window: Some(Window {
                    title: "ozmux".to_string(),
                    ime_enabled: true,
                    ..default()
                }),
                ..default()
            }),
            cef_plugin(dyn_registry.clone(), cef_profile.path()),
        ))
        .insert_state(initial_mode)
        .add_plugins((
            OzmaTerminalPlugin {
                config_shell: pre_configs.ozma.shell.clone(),
            },
            OzmaModePlugin,
            TerminalHandlePlugin,
            TerminalRendererPlugin,
            OzmuxTmuxPlugin,
            OzmuxPickerPlugin,
            OzmuxConfigsPlugin,
            FontBridgePlugin,
            OzmuxBootstrapPlugin,
            OzmuxShortcutPlugin,
            OzmuxUiPlugin,
            OzmuxWebviewPlugin {
                osc_enabled: pre_configs.osc_webview.enabled,
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
            OzmaHostInputPlugin,
            OzmuxControlPlanePlugin::new(dyn_registry),
        ))
        .run();
}
