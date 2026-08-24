//! Bevy integration for `orzma_tty`: the terminal handle component,
//! inbound request observers, and the outbound signal pump.

use crate::{requests::OrzmaEventRequestPlugin, signals::OrzmaTtySignalPlugin};
use bevy::prelude::*;
#[cfg(any(test, feature = "test-support"))]
use orzma_tty::test_support::CaptureSink;
use orzma_tty::{OrzmaTty, SpawnOptions, prelude::OrzmaTtyResult};
use orzma_vt::prelude::{GridSize, OrzmaVt};

mod requests;
mod signals;

pub mod prelude {
    pub use crate::{OrzmaTtyHandle, OrzmaTtyPlugin, requests::*, signals::*};
}

/// A live terminal owned by one Bevy entity: the PTY-backed
/// [`OrzmaTty`] driving an [`OrzmaVt`].
#[derive(Component, Deref, DerefMut)]
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

/// Registers the terminal signal pump and the inbound request
/// observers.
pub struct OrzmaTtyPlugin;

impl Plugin for OrzmaTtyPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((OrzmaTtySignalPlugin, OrzmaEventRequestPlugin));
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
