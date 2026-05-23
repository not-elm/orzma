//! ozmux Bevy GUI entry point.

mod bootstrap;
mod configs;
mod input;
mod multiplexer;
mod theme;
mod ui;

use bevy::prelude::*;
use bevy_inspector_egui::bevy_egui::EguiPlugin;
use bootstrap::OzmuxBootstrapPlugin;
use configs::OzmuxConfigsPlugin;
use input::OzmuxShortcutPlugin;
use multiplexer::OzmuxMultiplexerPlugin;
use multiplexer::log::OzmuxLayoutLogPlugin;
use ui::OzmuxUiPlugin;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins((
            EguiPlugin::default(),
            bevy_inspector_egui::quick::WorldInspectorPlugin::default(),
        ))
        .add_plugins((
            OzmuxMultiplexerPlugin,
            OzmuxConfigsPlugin,
            OzmuxLayoutLogPlugin,
            OzmuxBootstrapPlugin,
            OzmuxShortcutPlugin,
            OzmuxUiPlugin,
        ))
        .run();
}
