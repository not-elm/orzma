use bevy::prelude::*;

mod events;

pub mod prelude {
    pub use crate::OrzmaTerminalPlugin;
}

#[derive(Component, Deref, DerefMut)]
pub struct OrzmaTermHandle(orzma_terminal::OrzmaTerm<orzma_vt::Vt>);

pub struct OrzmaTerminalPlugin;

impl Plugin for OrzmaTerminalPlugin {
    fn build(app: &mut App) {
        app.add_systems
    }
}
