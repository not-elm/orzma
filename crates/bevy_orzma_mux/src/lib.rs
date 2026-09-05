//! Bevy integration for the multiplexer backend: the terminal handle
//! component, the title component, inbound request observers, and the
//! outbound signal pump. `MuxConnection`, `MuxPane`, and the `drain`
//! module bridge the same world to the out-of-process `orzma_mux`
//! backend, alongside the entity-owned `OrzmaTtyHandle` design until it
//! is removed.

use crate::{
    drain::DrainPlugin,
    layout::LayoutPlugin,
    requests::OrzmaEventRequestPlugin,
    signals::OrzmaTtySignalPlugin,
    title::{TtyTitle, TtyTitlePlugin},
};
use bevy::prelude::*;
use orzma_mux::prelude::{MuxClient, PaneId};
#[cfg(any(test, feature = "test-support"))]
use orzma_tty::test_support::CaptureSink;
use orzma_tty::{OrzmaTty, SpawnOptions, prelude::OrzmaTtyResult};
use orzma_vt::prelude::{GridSize, OrzmaVt};

mod drain;
mod layout;
mod registry;
mod requests;
mod signals;
mod title;

pub mod prelude {
    pub use crate::{
        MuxConnection, MuxPane, MuxSystems, OrzmaMuxPlugin, OrzmaTtyHandle, OrzmaTtyPlugin,
        drain::{MuxPaneSpawnFailed, MuxSessionEnded},
        layout::{CurrentLayout, MuxActivePaneChanged, MuxSeparator, PaneGeometry, pane_node},
        registry::PaneRegistry,
        requests::*,
        signals::*,
        title::TtyTitle,
    };
    pub use orzma_mux::prelude::{MuxClient, MuxConfig, MuxSpawnError};
}

/// A live terminal owned by one Bevy entity: the PTY-backed
/// [`OrzmaTty`] driving an [`OrzmaVt`].
#[derive(Component, Deref, DerefMut)]
#[require(TtyTitle)]
pub struct OrzmaTtyHandle(OrzmaTty<OrzmaVt>);

impl OrzmaTtyHandle {
    /// Scrollback rows every terminal retains on its primary screen.
    const MAX_HISTORY: usize = 10_000;

    /// Spawns the login shell under a new PTY and wraps it in a handle.
    pub fn new(options: SpawnOptions) -> OrzmaTtyResult<Self> {
        let vt = OrzmaVt::new(
            GridSize {
                cols: options.cols,
                rows: options.rows,
            },
            Self::MAX_HISTORY,
        );
        Ok(Self(OrzmaTty::spawn(vt, options)?))
    }
}

/// Registers the terminal signal pump, the inbound request observers,
/// and the title component's observers.
pub struct OrzmaTtyPlugin;

impl Plugin for OrzmaTtyPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            OrzmaTtySignalPlugin,
            OrzmaEventRequestPlugin,
            TtyTitlePlugin,
        ));
    }
}

#[cfg(any(test, feature = "test-support"))]
impl OrzmaTtyHandle {
    /// Builds a handle around a PTY-less terminal whose writes land on
    /// the returned [`CaptureSink`], so tests can assert the exact bytes
    /// the observers put on the PTY write seam.
    pub fn detached(cols: u16, rows: u16) -> (Self, CaptureSink) {
        let sink = CaptureSink::default();
        let vt = OrzmaVt::new(GridSize { cols, rows }, Self::MAX_HISTORY);
        let term = OrzmaTty::detached(vt, cols, rows, Box::new(sink.clone()))
            .expect("OrzmaTty::detached failed");
        (Self(term), sink)
    }
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
///
/// `OrzmaEventRequestPlugin` is left out here for now: `OrzmaTtyPlugin`
/// still registers it, and adding both plugins to the same app would
/// double the request observers. Task 10 moves that registration here
/// once the requests read `MuxConnection` instead of `OrzmaTtyHandle`.
pub struct OrzmaMuxPlugin;

impl Plugin for OrzmaMuxPlugin {
    fn build(&self, app: &mut App) {
        app.configure_sets(Update, (MuxSystems::Drain, MuxSystems::ApplyLayout).chain())
            .add_plugins((DrainPlugin, LayoutPlugin, TtyTitlePlugin));
    }
}
