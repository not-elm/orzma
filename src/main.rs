//! ozmux Bevy GUI entry point.

mod bootstrap;
mod configs;
mod control_plane;
mod font;
mod inline_webview;
mod input;
mod osc_webview;
mod ozma;
mod ozma_input;
mod picker;
mod system_set;
mod theme;
mod tmux;
mod ui;
mod webview_render;

use crate::control_plane::OzmuxControlPlanePlugin;
use crate::inline_webview::OzmuxInlineWebviewPlugin;
use crate::input::hyperlink::HyperlinkInputPlugin;
use crate::osc_webview::OzmuxOscWebviewPlugin;
use crate::ozma::{AppMode, OzmaModePlugin};
use crate::webview_render::{OzmuxWebviewRenderPlugin, cef_plugin};
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
use ozmux_webview_host::DynAssetRegistry;
use picker::OzmuxPickerPlugin;
use tmux::OzmuxTmuxPlugin;
use ui::ime_overlay::ImeOverlayPlugin;
use ui::{
    OzmuxUiPlugin, confirm_prompt::ConfirmPromptPlugin, copy_mode::CopyModePlugin,
    copy_mode_indicator::CopyModeIndicatorPlugin, copy_search::CopyPromptPlugin,
    rename_prompt::RenamePromptPlugin,
};

fn main() {
    let pre_configs = ozmux_configs::OzmuxConfigs::load_blocking().unwrap_or_default();
    // NOTE: Always start in AppMode::Ozmux so Startup deferred commands
    // (e.g. init_atlas_image inserting AtlasImage) are flushed before any
    // OnEnter(AppMode::Ozma) fires. on_enter_ozmux_picker handles
    // StartupMode::Ozma by immediately transitioning to AppMode::Ozma.
    let initial_mode = match pre_configs.startup_mode {
        StartupMode::Ozma | StartupMode::Ozmux | StartupMode::AutoAttach => AppMode::Ozmux,
    };
    let dyn_registry = DynAssetRegistry::default();
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
            cef_plugin(dyn_registry.clone()),
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
            OzmuxWebviewRenderPlugin,
            CopyModePlugin,
            CopyModeIndicatorPlugin,
        ))
        .add_plugins(CopyPromptPlugin)
        .add_plugins(ConfirmPromptPlugin)
        .add_plugins(RenamePromptPlugin)
        .add_plugins((
            HyperlinkInputPlugin,
            ImePlugin,
            ImeOverlayPlugin,
            OptionAsAltPlugin,
            OzmaHostInputPlugin,
            OzmuxOscWebviewPlugin,
            OzmuxInlineWebviewPlugin,
            OzmuxControlPlanePlugin::new(dyn_registry),
        ))
        .run();
}
