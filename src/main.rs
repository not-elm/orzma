//! ozmux Bevy GUI entry point.

mod bootstrap;
mod clipboard;
mod configs;
mod extension_render;
mod font;
mod input;
mod multiplexer;
mod system_set;
mod theme;
mod ui;

use crate::extension_render::{AssetEndpoint, OzmuxExtensionRenderPlugin, cef_plugin};
use crate::input::hyperlink::HyperlinkInputPlugin;
use crate::input::mouse_buttons::MouseButtonsInputPlugin;
use crate::input::mouse_wheel::MouseWheelInputPlugin;
use crate::multiplexer::commands::OzmuxShortcutActionPlugin;
use bevy::prelude::*;
use bevy_terminal::TerminalHandlePlugin;
use bevy_terminal_renderer::TerminalRendererPlugin;
use bootstrap::OzmuxBootstrapPlugin;
use configs::OzmuxConfigsPlugin;
use font::FontBridgePlugin;
use input::OzmuxShortcutPlugin;
use input::ime::ImePlugin;
use multiplexer::log::OzmuxLayoutLogPlugin;
use ozmux_extension_host::{CommandExtensionConfig, ExtensionControlPlugin};
use ozmux_multiplexer::MultiplexerPlugin;
use ui::ime_overlay::ImeOverlayPlugin;
use ui::{OzmuxUiPlugin, copy_mode::CopyModePlugin, copy_mode_indicator::CopyModeIndicatorPlugin};

fn main() {
    let asset_endpoint = AssetEndpoint::default();
    App::new()
        .add_plugins((
            DefaultPlugins.set(WindowPlugin {
                primary_window: Some(Window {
                    title: "ozmux".to_string(),
                    ..default()
                }),
                ..default()
            }),
            cef_plugin(&asset_endpoint),
        ))
        .insert_resource(asset_endpoint)
        .add_plugins((
            TerminalHandlePlugin,
            TerminalRendererPlugin,
            MultiplexerPlugin,
            OzmuxConfigsPlugin,
            FontBridgePlugin,
            OzmuxLayoutLogPlugin,
            OzmuxBootstrapPlugin,
            OzmuxShortcutPlugin,
            OzmuxUiPlugin,
            OzmuxExtensionRenderPlugin,
            CopyModePlugin,
            CopyModeIndicatorPlugin,
        ))
        .add_plugins((
            MouseWheelInputPlugin,
            MouseButtonsInputPlugin,
            HyperlinkInputPlugin,
            ImePlugin,
            ImeOverlayPlugin,
            OzmuxShortcutActionPlugin,
            ExtensionControlPlugin::new(CommandExtensionConfig {
                name: "memo".into(),
                dir: std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("extensions/memo"),
                main: "bootstrap.ts".into(),
                commands: vec!["@memo".into()],
            }),
        ))
        // .insert_resource(WinitSettings::desktop_app())
        .run();
}
