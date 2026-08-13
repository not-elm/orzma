use bevy::prelude::*;
#[cfg(test)]
use orzma_term::test_support::CaptureSink;
use orzma_term::{OrzmaTerm, SpawnOptions, prelude::OrzmaTermResult};
use orzma_vt::prelude::AlacrittyVtBackend;

use crate::{requests::OrzmaEventRequestPlugin, signals::OrzmaTermSignalPlugin};

mod requests;
mod signals;

pub mod prelude {
    pub use crate::{OrzmaTermHandle, OrzmaTerminalPlugin, requests::*, signals::*};
}

#[derive(Component, Deref, DerefMut)]
pub struct OrzmaTermHandle(OrzmaTerm<AlacrittyVtBackend>);

impl OrzmaTermHandle {
    pub fn new(options: SpawnOptions) -> OrzmaTermResult<Self> {
        Ok(Self(OrzmaTerm::spawn(options)?))
    }
}

pub struct OrzmaTerminalPlugin;

impl Plugin for OrzmaTerminalPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((OrzmaTermSignalPlugin, OrzmaEventRequestPlugin));
    }
}

#[cfg(test)]
impl OrzmaTermHandle {
    /// Builds a handle around a PTY-less terminal whose writes land on
    /// the returned [`CaptureSink`], so tests can assert the exact bytes
    /// the observers put on the PTY write seam.
    fn detached(cols: u16, rows: u16) -> (Self, CaptureSink) {
        let sink = CaptureSink::default();
        let term = OrzmaTerm::detached(cols, rows, Box::new(sink.clone()))
            .expect("OrzmaTerm::detached failed");
        (Self(term), sink)
    }
}
