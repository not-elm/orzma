use bevy::prelude::*;
use orzma_term::{OrzmaTerm, SpawnOptions, prelude::OrzmaTermResult};
use orzma_vt::prelude::AlacrittyVt;

mod events;

pub mod prelude {
    pub use crate::{OrzmaTermHandle, OrzmaTerminalPlugin};
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
        todo!()
    }
}
