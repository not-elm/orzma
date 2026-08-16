//! Bevy integration for `orzma_term`: the terminal handle component,
//! inbound request observers, and the outbound signal pump.

use crate::{requests::OrzmaEventRequestPlugin, signals::OrzmaTermSignalPlugin};
use bevy::prelude::*;
#[cfg(any(test, feature = "test-support"))]
use orzma_term::test_support::CaptureSink;
use orzma_term::{OrzmaTerm, SpawnOptions, prelude::OrzmaTermResult};
use orzma_vt::prelude::{AlacrittyVtBackend, OldOrzmaVt};

mod requests;
mod signals;

pub mod prelude {
    pub use crate::{OrzmaTermHandle, OrzmaTerminalPlugin, requests::*, signals::*};
}

/// A live terminal owned by one Bevy entity: the PTY-backed
/// [`OrzmaTerm`] driving the legacy alacritty-backed VT.
#[derive(Component, Deref, DerefMut)]
pub struct OrzmaTermHandle(OrzmaTerm<OldOrzmaVt<AlacrittyVtBackend>>);

impl OrzmaTermHandle {
    /// Spawns the login shell under a new PTY and wraps it in a handle.
    pub fn new(options: SpawnOptions) -> OrzmaTermResult<Self> {
        let vt = OldOrzmaVt::<AlacrittyVtBackend>::new(options.cols, options.rows);
        Ok(Self(OrzmaTerm::spawn(vt, options)?))
    }
}

/// Registers the terminal signal pump and the inbound request
/// observers.
pub struct OrzmaTerminalPlugin;

impl Plugin for OrzmaTerminalPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((OrzmaTermSignalPlugin, OrzmaEventRequestPlugin));
    }
}

#[cfg(any(test, feature = "test-support"))]
impl OrzmaTermHandle {
    /// Builds a handle around a PTY-less terminal whose writes land on
    /// the returned [`CaptureSink`], so tests can assert the exact bytes
    /// the observers put on the PTY write seam.
    pub fn detached(cols: u16, rows: u16) -> (Self, CaptureSink) {
        let sink = CaptureSink::default();
        let vt = OldOrzmaVt::<AlacrittyVtBackend>::new(cols, rows);
        let term = OrzmaTerm::detached(vt, cols, rows, Box::new(sink.clone()))
            .expect("OrzmaTerm::detached failed");
        (Self(term), sink)
    }
}
