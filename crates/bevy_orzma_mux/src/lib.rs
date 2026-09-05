//! Bevy integration for the multiplexer backend: the `MuxPane` component
//! every pane entity carries, the title component, inbound request
//! observers, and the outbound signal types the drain triggers.
//! `MuxConnection`, `MuxPane`, and the `drain` module bridge the same
//! world to the multiplexer backend thread.

use crate::{
    drain::DrainPlugin,
    layout::LayoutPlugin,
    requests::OrzmaEventRequestPlugin,
    title::{TtyTitle, TtyTitlePlugin},
};
use bevy::prelude::*;
use orzma_mux::prelude::{MuxClient, PaneId};

mod drain;
mod layout;
mod registry;
mod requests;
mod signals;
mod title;

pub mod prelude {
    pub use crate::{
        MuxConnection, MuxPane, MuxSystems, OrzmaMuxPlugin,
        drain::{MuxPaneSpawnFailed, MuxSessionEnded},
        layout::{CurrentLayout, MuxActivePaneChanged, MuxSeparator, PaneGeometry, pane_node},
        registry::PaneRegistry,
        requests::*,
        signals::*,
        title::TtyTitle,
    };
    pub use orzma_mux::prelude::{MuxClient, MuxConfig, MuxSpawnError};
}

/// The GUI's connection to the multiplexer backend.
#[derive(Resource)]
pub struct MuxConnection(pub MuxClient);

/// The backend pane an entity mirrors. Present from `PaneOpened` until
/// the entity despawns on `PaneClosed`.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
#[require(TtyTitle)]
pub struct MuxPane(pub PaneId);

/// Ordering slots for the bridge's `Update` systems. The host chains
/// `Drain → ApplyLayout → its input phases`.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum MuxSystems {
    /// `drain_mux_events`.
    Drain,
    /// `apply_layout`.
    ApplyLayout,
}

/// Registers the drain, the layout applier, the request observers, and
/// the title component's observers.
pub struct OrzmaMuxPlugin;

impl Plugin for OrzmaMuxPlugin {
    fn build(&self, app: &mut App) {
        app.configure_sets(Update, (MuxSystems::Drain, MuxSystems::ApplyLayout).chain())
            .add_plugins((
                DrainPlugin,
                LayoutPlugin,
                OrzmaEventRequestPlugin,
                TtyTitlePlugin,
            ));
    }
}
