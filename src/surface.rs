//! Shared terminal-surface identity: the `OzmaTerminal` marker and the
//! render-bundle injection observer, which fire for every surface — tmux
//! panes and the Default-mode shell alike. Surface geometry helpers live in
//! `geom`.

pub(crate) mod geom;

use bevy::prelude::*;
use ozma_tty_renderer::material::TerminalUiMaterial;
use ozma_tty_renderer::prelude::TerminalRenderBundle;

/// Marker component identifying an Ozma-mode terminal entity.
///
/// One or more entities may carry this marker; mouse input routes to the
/// topmost under the cursor, while keyboard input (raw keys and IME) targets the
/// single entity the host marks `KeyboardFocused`.
#[derive(Component)]
pub(crate) struct OzmaTerminal;

/// Registers the render-bundle injection observer.
pub(crate) struct SurfacePlugin;

impl Plugin for SurfacePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_add_inject_render);
    }
}

/// Bevy observer that injects a `TerminalRenderBundle` whenever `OzmaTerminal`
/// is added to an entity, allocating the GPU material on demand.
fn on_add_inject_render(
    ev: On<Add, OzmaTerminal>,
    mut commands: Commands,
    mut materials: ResMut<Assets<TerminalUiMaterial>>,
) {
    let material = materials.add(TerminalUiMaterial::default());
    commands
        .entity(ev.event_target())
        .insert(TerminalRenderBundle::new(material));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_add_injects_render_bundle() {
        use bevy::asset::AssetPlugin;
        use ozma_tty_renderer::schema::TerminalGrid;

        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()));
        app.init_resource::<Assets<TerminalUiMaterial>>();
        app.add_observer(on_add_inject_render);
        let entity = app.world_mut().spawn(OzmaTerminal).id();
        app.update();
        assert!(
            app.world().entity(entity).contains::<TerminalGrid>(),
            "On<Add, OzmaTerminal> must inject TerminalRenderBundle (TerminalGrid)",
        );
    }
}
