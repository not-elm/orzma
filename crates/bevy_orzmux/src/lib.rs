//! Bevy integration for the multiplexer backend: the `OrzmuxPane` component
//! every pane entity carries, the title component, inbound request
//! observers, and the outbound signal types the drain triggers.
//! `OrzmuxConnection`, `OrzmuxPane`, and the `drain` module bridge the same
//! world to the multiplexer backend thread.

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
#[derive(Resource)]
pub struct OrzmuxConnection(pub OrzmuxClient);

/// The backend pane an entity mirrors. Present from `PaneOpened` until
/// the entity despawns on `PaneClosed`.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
#[require(TtyTitle)]
pub struct OrzmuxPane(pub PaneId);

/// Ordering slots for the bridge's `Update` systems. The host chains
/// `Drain → ApplyLayout → its input phases`.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum OrzmuxSystems {
    /// `drain_orzmux_events`.
    Drain,
    /// `apply_layout`.
    ApplyLayout,
}

/// Registers the drain, the layout applier, the request observers, and
/// the title component's observers.
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
