//! Bevy integration for the multiplexer backend: mirrors its panes and
//! layout into the world, forwards the host's requests to it, and turns
//! its events into ECS signals.

use crate::{
    drain::DrainPlugin,
    layout::LayoutPlugin,
    requests::OrzmaEventRequestPlugin,
    title::{TtyTitle, TtyTitlePlugin},
};
use bevy::prelude::*;
use orzmux::prelude::{OrzmuxClient, PaneId};

mod drain;
mod layout;
mod registry;
mod requests;
mod signals;
mod title;

pub mod prelude {
    pub use crate::{
        OrzmuxConnection, OrzmuxPane, OrzmuxPlugin, OrzmuxSystems,
        drain::{OrzmuxPaneSpawnFailed, OrzmuxSessionEnded},
        layout::{OrzmuxActivePaneChanged, OrzmuxPaneContainer, PaneGeometry, absolute_px_node},
        registry::PaneRegistry,
        requests::*,
        signals::*,
        title::TtyTitle,
    };
    pub use orzmux::prelude::{OrzmuxClient, OrzmuxConfig, OrzmuxSpawnError};
}

/// The GUI's connection to the multiplexer backend.
///
/// # Invariants
///
/// The resource exists only until the drain detects that the backend is
/// gone; the drain then removes it. Every system and observer that reads it
/// is therefore gated with `run_if(resource_exists::<OrzmuxConnection>)`,
/// because a missing `Res` panics under Bevy's default error handler.
#[derive(Resource)]
pub struct OrzmuxConnection(pub OrzmuxClient);

/// The backend pane an entity mirrors. Present from `PaneOpened` until
/// the entity despawns on `PaneClosed`.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
#[require(TtyTitle)]
pub struct OrzmuxPane(pub PaneId);

/// Ordering slots for the bridge's `Update` systems: `Drain` runs before
/// `ApplyLayout`, and a host orders its input phases after both.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum OrzmuxSystems {
    /// Draining the backend's events into the world.
    Drain,
    /// Applying the latest layout to the pane nodes.
    ApplyLayout,
}

/// Mirrors the backend's panes, layout, and titles into the app, and
/// forwards the host's requests to the backend.
pub struct OrzmuxPlugin;

impl Plugin for OrzmuxPlugin {
    fn build(&self, app: &mut App) {
        app.configure_sets(
            Update,
            (OrzmuxSystems::Drain, OrzmuxSystems::ApplyLayout).chain(),
        )
        .add_plugins((
            DrainPlugin,
            LayoutPlugin,
            OrzmaEventRequestPlugin,
            TtyTitlePlugin,
        ));
    }
}
