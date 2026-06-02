//! ozmux Bevy GUI entry point.

mod action;
mod bootstrap;
mod browser_render;
mod clipboard;
mod configs;
mod extension_manager;
mod extension_render;
mod font;
mod input;
mod multiplexer;
mod system_set;
mod theme;
mod ui;

use crate::action::OzmuxActionPlugin;
use crate::browser_render::OzmuxBrowserRenderPlugin;
use crate::clipboard::ClipboardActionPlugin;
use crate::extension_manager::ExtensionManagerPlugin;
use crate::extension_render::{OzmuxExtensionRenderPlugin, cef_plugin};
use crate::input::hyperlink::HyperlinkInputPlugin;
use crate::input::mouse_buttons::MouseButtonsInputPlugin;
use crate::input::mouse_wheel::MouseWheelInputPlugin;
use bevy::prelude::*;
use bevy_terminal::TerminalHandlePlugin;
use bevy_terminal_renderer::TerminalRendererPlugin;
use bootstrap::OzmuxBootstrapPlugin;
use configs::OzmuxConfigsPlugin;
use font::FontBridgePlugin;
use input::OzmuxShortcutPlugin;
use input::ime::ImePlugin;
use multiplexer::log::OzmuxLayoutLogPlugin;
use ozmux_extension_host::host::EndpointRegistry;
use ozmux_multiplexer::MultiplexerPlugin;
use ui::ime_overlay::ImeOverlayPlugin;
use ui::{
    OzmuxUiPlugin, copy_mode::CopyModePlugin, copy_mode_indicator::CopyModeIndicatorPlugin,
    tab_input::TabInteractionPlugin,
};

fn main() {
    let endpoints = EndpointRegistry::default();
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
            cef_plugin(endpoints.clone()),
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
            OzmuxBrowserRenderPlugin,
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
            ExtensionManagerPlugin::new(endpoints),
            OzmuxActionPlugin,
        ))
        .run();
}
