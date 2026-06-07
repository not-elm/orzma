//! ozmux Bevy GUI entry point.

mod action;
mod bootstrap;
mod browser_render;
mod clipboard;
mod configs;
mod extension_manager;
mod extension_render;
mod font;
mod geometry_feed;
mod input;
#[cfg(not(feature = "thin-client"))]
mod multiplexer;
mod system_set;
mod theme;
#[cfg(feature = "thin-client")]
mod thin_client;
mod ui;

use crate::action::OzmuxActionPlugin;
use crate::browser_render::OzmuxBrowserRenderPlugin;
use crate::clipboard::ClipboardActionPlugin;
use crate::extension_manager::ExtensionManagerPlugin;
use crate::extension_render::{OzmuxExtensionRenderPlugin, cef_plugin};
use crate::geometry_feed::GeometryFeedPlugin;
use crate::input::hyperlink::HyperlinkInputPlugin;
use crate::input::mouse_buttons::MouseButtonsInputPlugin;
use crate::input::mouse_wheel::MouseWheelInputPlugin;
use bevy::prelude::*;
#[cfg(not(feature = "thin-client"))]
use bevy_terminal::TerminalHandlePlugin;
use bevy_terminal_renderer::TerminalRendererPlugin;
use bootstrap::OzmuxBootstrapPlugin;
use configs::OzmuxConfigsPlugin;
use font::FontBridgePlugin;
use input::OzmuxShortcutPlugin;
use input::ime::ImePlugin;
#[cfg(not(feature = "thin-client"))]
use multiplexer::log::OzmuxLayoutLogPlugin;
use ozmux_extension_host::host::EndpointRegistry;
#[cfg(not(feature = "thin-client"))]
use ozmux_multiplexer::MultiplexerPlugin;
use ui::ime_overlay::ImeOverlayPlugin;
use ui::{
    OzmuxUiPlugin, copy_mode::CopyModePlugin, copy_mode_indicator::CopyModeIndicatorPlugin,
    tab_input::TabInteractionPlugin,
};

fn main() {
    let endpoints = EndpointRegistry::default();
    let mut app = App::new();
    app.add_plugins((
        DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "ozmux".to_string(),
                ime_enabled: true,
                ..default()
            }),
            ..default()
        }),
        cef_plugin(endpoints.clone()),
    ));

    // Local-only plugins (PTY, mux write-path).
    #[cfg(not(feature = "thin-client"))]
    app.add_plugins((
        TerminalHandlePlugin,
        MultiplexerPlugin,
        OzmuxLayoutLogPlugin,
    ));

    // Thin-client plugin (wire read-path + in-process daemon).
    #[cfg(feature = "thin-client")]
    app.add_plugins(thin_client::ThinClientMultiplexerPlugin);

    // Bootstrap is shared: the local build registers mux-seed + cursor; the
    // thin-client build registers cursor only (workspace is seeded by
    // ThinClientMultiplexerPlugin).
    app.add_plugins(OzmuxBootstrapPlugin);

    // Shared plugins (render, UI, input, configs, font, shortcuts, etc.).
    app.add_plugins((
        TerminalRendererPlugin,
        OzmuxConfigsPlugin,
        FontBridgePlugin,
        OzmuxShortcutPlugin,
        OzmuxUiPlugin,
        OzmuxExtensionRenderPlugin,
        OzmuxBrowserRenderPlugin,
        CopyModePlugin,
        ClipboardActionPlugin,
        CopyModeIndicatorPlugin,
        TabInteractionPlugin,
    ));
    app.add_plugins((
        MouseWheelInputPlugin,
        MouseButtonsInputPlugin,
        HyperlinkInputPlugin,
        ImePlugin,
        ImeOverlayPlugin,
        ExtensionManagerPlugin::new(endpoints),
    ));

    app.add_plugins(OzmuxActionPlugin);

    app.add_plugins(GeometryFeedPlugin);
    app.run();
}
