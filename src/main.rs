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
mod tmux_boot;
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
use bootstrap::OzmuxBootstrapPlugin;
use configs::OzmuxConfigsPlugin;
use font::FontBridgePlugin;
use input::OzmuxShortcutPlugin;
use input::ime::ImePlugin;
use multiplexer::log::OzmuxLayoutLogPlugin;
use ozma_tty_engine::TerminalHandlePlugin;
use ozma_tty_renderer::TerminalRendererPlugin;
use ozmux_extension_host::DynAssetRegistry;
use ozmux_multiplexer::MultiplexerPlugin;
use ozmux_tmux::TmuxSessionPlugin;
use tmux_boot::TmuxBootPlugin;
use ui::ime_overlay::ImeOverlayPlugin;
use ui::tmux_dialog::TmuxDialogPlugin;
use ui::{
    OzmuxUiPlugin, copy_mode::CopyModePlugin, copy_mode_indicator::CopyModeIndicatorPlugin,
    tab_input::TabInteractionPlugin,
};

fn main() {
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
        .add_plugins((
            TerminalHandlePlugin,
            TerminalRendererPlugin,
            MultiplexerPlugin,
            TmuxSessionPlugin,
            TmuxBootPlugin,
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
        ))
        .add_plugins(TabInteractionPlugin)
        .add_plugins(TmuxDialogPlugin)
        .add_plugins((
            MouseWheelInputPlugin,
            MouseButtonsInputPlugin,
            HyperlinkInputPlugin,
            ImePlugin,
            ImeOverlayPlugin,
            OzmuxOscWebviewPlugin,
            OzmuxInlineWebviewPlugin,
            ExtensionManagerPlugin,
            OzmuxControlPlanePlugin::new(dyn_registry),
            OzmuxActionPlugin,
        ))
        .run();
}
