//! ozmux Bevy GUI entry point.

mod bootstrap;
mod clipboard;
mod configs;
mod font;
mod input;
mod multiplexer;
mod theme;
mod ui;

use bevy::prelude::*;
use bevy_terminal::TerminalHandlePlugin;
use bevy_terminal_renderer::TerminalRendererPlugin;
use bootstrap::OzmuxBootstrapPlugin;
use configs::OzmuxConfigsPlugin;
use font::FontBridgePlugin;
use input::OzmuxShortcutPlugin;
use multiplexer::OzmuxMultiplexerPlugin;
use multiplexer::log::OzmuxLayoutLogPlugin;
use ui::{OzmuxUiPlugin, copy_mode::CopyModePlugin, copy_mode_indicator::CopyModeIndicatorPlugin};

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins((
            TerminalHandlePlugin,
            TerminalRendererPlugin,
            OzmuxMultiplexerPlugin,
            OzmuxConfigsPlugin,
            FontBridgePlugin,
            OzmuxLayoutLogPlugin,
            OzmuxBootstrapPlugin,
            OzmuxShortcutPlugin,
            OzmuxUiPlugin,
            CopyModePlugin,
            CopyModeIndicatorPlugin,
            crate::input::mouse_wheel::MouseWheelInputPlugin,
        ))
        .run();
}
