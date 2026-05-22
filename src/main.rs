//! ozmux Bevy GUI entry point.

mod bootstrap;
mod input;
mod multiplexer;
mod theme;
mod ui;

use bevy::prelude::*;
use bootstrap::OzmuxBootstrapPlugin;
use input::OzmuxShortcutPlugin;
use multiplexer::OzmuxMultiplexerPlugin;
use multiplexer::log::OzmuxLayoutLogPlugin;
use ui::OzmuxUiPlugin;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins((
            OzmuxMultiplexerPlugin,
            OzmuxLayoutLogPlugin,
            OzmuxBootstrapPlugin,
            OzmuxShortcutPlugin,
            OzmuxUiPlugin,
        ))
        .run();
}
