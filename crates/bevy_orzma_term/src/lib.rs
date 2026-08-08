use bevy::prelude::*;
use orzma_term::{OrzmaTerm, SpawnOptions, prelude::OrzmaTermResult};
use orzma_vt::prelude::AlacrittyVt;

use crate::{requests::OrzmaEventRequestPlugin, signals::OrzmaTermSignalPlugin};

mod requests;
mod signals;

pub mod prelude {
    pub use crate::{OrzmaTermHandle, OrzmaTerminalPlugin, requests::*, signals::*};
}

#[derive(Component, Deref, DerefMut)]
pub struct OrzmaTermHandle(OrzmaTerm<AlacrittyVt>);

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
