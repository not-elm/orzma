//! ozmux Bevy GUI entry point.

mod action;
mod bootstrap;
mod clipboard;
mod configs;
mod control_plane;
mod extension_manager;
mod extension_render;
mod font;
mod inline_webview;
mod input;
mod multiplexer;
mod osc_webview;
mod system_set;
mod theme;
mod ui;

use crate::action::OzmuxActionPlugin;
use crate::clipboard::ClipboardActionPlugin;
use crate::control_plane::OzmuxControlPlanePlugin;
use crate::extension_manager::ExtensionManagerPlugin;
use crate::extension_render::{OzmuxExtensionRenderPlugin, cef_plugin};
use crate::inline_webview::OzmuxInlineWebviewPlugin;
use crate::input::hyperlink::HyperlinkInputPlugin;
use crate::input::mouse_buttons::MouseButtonsInputPlugin;
use crate::input::mouse_wheel::MouseWheelInputPlugin;
use crate::osc_webview::OzmuxOscWebviewPlugin;
use bevy::prelude::*;
use bevy_terminal::TerminalHandlePlugin;
use bevy_terminal_renderer::TerminalRendererPlugin;
use bootstrap::OzmuxBootstrapPlugin;
use configs::OzmuxConfigsPlugin;
use font::FontBridgePlugin;
use input::OzmuxShortcutPlugin;
use input::ime::ImePlugin;
use multiplexer::log::OzmuxLayoutLogPlugin;
use ozmux_extension_host::DynAssetRegistry;
use ozmux_extension_host::host::AssetSourceRegistry;
use ozmux_multiplexer::MultiplexerPlugin;
use ui::ime_overlay::ImeOverlayPlugin;
use ui::{
    OzmuxUiPlugin, copy_mode::CopyModePlugin, copy_mode_indicator::CopyModeIndicatorPlugin,
    tab_input::TabInteractionPlugin,
};

fn main() {
    let registry = AssetSourceRegistry::default();
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
            cef_plugin(registry.clone(), dyn_registry.clone()),
        ))
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
            ClipboardActionPlugin,
            CopyModeIndicatorPlugin,
            TabInteractionPlugin,
        ))
        .add_plugins((
            MouseWheelInputPlugin,
            MouseButtonsInputPlugin,
            HyperlinkInputPlugin,
            ImePlugin,
            ImeOverlayPlugin,
            OzmuxOscWebviewPlugin,
            OzmuxInlineWebviewPlugin,
            ExtensionManagerPlugin::new(registry),
            OzmuxControlPlanePlugin::new(dyn_registry),
            OzmuxActionPlugin,
        ))
        .run();
}
