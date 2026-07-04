//! tmux-mode input dispatch: keyboard forwarding, mouse gestures, per-pane
//! input gates, IME/mouse forwarding to tmux, pane hit-testing, and window-bar
//! input. The complementary tmux state and rendering live in
//! `crate::ui::tmux` / `crate::render::tmux` / `crate::session::tmux`.

pub(crate) mod forward;
mod gate;
mod input;
pub(crate) mod mouse;
mod pane_hit;
mod window_bar_input;

use bevy::prelude::*;
use forward::ForwardPlugin;
use gate::GatePlugin;
use input::InputPlugin;
use mouse::MousePlugin;
use window_bar_input::WindowBarInputPlugin;

/// Bevy plugin aggregating the tmux-mode input sub-plugins.
pub(super) struct TmuxInputPlugin;

impl Plugin for TmuxInputPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            InputPlugin,
            MousePlugin,
            ForwardPlugin,
            WindowBarInputPlugin,
            GatePlugin,
        ));
    }
}
