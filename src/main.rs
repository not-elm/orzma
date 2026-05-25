//! ozmux Bevy GUI entry point.

mod bootstrap;
mod configs;
mod input;
mod multiplexer;
mod theme;
mod ui;

use bevy::prelude::*;
use bevy_terminal::TerminalHandlePlugin;
use bevy_terminal_renderer::TerminalRendererPlugin;
use bootstrap::OzmuxBootstrapPlugin;
use configs::OzmuxConfigsPlugin;
use input::OzmuxShortcutPlugin;
use multiplexer::OzmuxMultiplexerPlugin;
use multiplexer::log::OzmuxLayoutLogPlugin;
use ui::{OzmuxUiPlugin, copy_mode::CopyModePlugin};

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins((
            TerminalHandlePlugin,
            TerminalRendererPlugin,
            OzmuxMultiplexerPlugin,
            OzmuxConfigsPlugin,
            OzmuxLayoutLogPlugin,
            OzmuxBootstrapPlugin,
            OzmuxShortcutPlugin,
            OzmuxUiPlugin,
            CopyModePlugin,
        ))
        .run();
}
