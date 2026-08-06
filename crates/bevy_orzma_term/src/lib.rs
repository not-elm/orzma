use bevy::prelude::*;
use orzma_term::OrzmaTerm;
use orzma_vt::prelude::AlacrittyVt;

mod events;

pub mod prelude {
    pub use crate::OrzmaTerminalPlugin;
}

#[derive(Component, Deref, DerefMut)]
pub struct OrzmaTermHandle(OrzmaTerm<AlacrittyVt>);

pub struct OrzmaTerminalPlugin;

impl Plugin for OrzmaTerminalPlugin {
    fn build(app: &mut App) {
        todo!("Implement OrzmaTerminalPlugin::build")
    }
}
