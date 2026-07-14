//! Shared terminal-surface identity: the `OrzmaTerminal` marker and the
//! render-bundle injection observer, which fire for every surface. Surface
//! geometry helpers live in `geometry`.

pub(crate) mod geometry;

use bevy::prelude::*;
use orzma_tty_engine::TerminalHandle;
use orzma_tty_renderer::material::TerminalUiMaterial;
use orzma_tty_renderer::prelude::TerminalRenderBundle;
use orzma_tty_renderer::schema::TerminalGrid;

/// Marker component identifying an Orzma-mode terminal entity.
///
/// One or more entities may carry this marker; mouse input routes to the
/// topmost under the cursor, while keyboard input (raw keys and IME) targets the
/// single entity the host marks `KeyboardFocused`.
#[derive(Component)]
pub(crate) struct OrzmaTerminal;

/// Registers the render-bundle injection observer.
pub(crate) struct SurfacePlugin;

impl Plugin for SurfacePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_add_inject_render)
            .add_observer(on_add_grid_repaint);
    }
}

/// Bevy observer that injects a `TerminalRenderBundle` whenever `OrzmaTerminal`
/// is added to an entity, allocating the GPU material on demand.
fn on_add_inject_render(
    ev: On<Add, OrzmaTerminal>,
    mut commands: Commands,
    mut materials: ResMut<Assets<TerminalUiMaterial>>,
) {
    let material = materials.add(TerminalUiMaterial::default());
    commands
        .entity(ev.event_target())
        .insert(TerminalRenderBundle::new(material));
}

/// Bevy observer that forces a full engine repaint when a terminal's
/// `TerminalGrid` is (late-)added.
///
/// The render bundle (which carries `TerminalGrid`) is injected by
/// `on_add_inject_render`, a *deferred* observer. A terminal spawned
/// mid-session — e.g. a multiplexer pane split — can have its engine emit its
/// first frame before that grid lands; `apply_snapshot` then silently drops
/// that frame and later `FrameDelta`s cannot rebuild the still-empty grid, so
/// the pane renders black forever. Re-emitting the handle's full current state
/// the moment the grid appears closes that race regardless of injection timing
/// (the bootstrap pane only dodged it by the shell's startup latency).
fn on_add_grid_repaint(
    ev: On<Add, TerminalGrid>,
    mut commands: Commands,
    mut terminals: Query<&mut TerminalHandle>,
) {
    let entity = ev.event_target();
    if let Ok(mut handle) = terminals.get_mut(entity) {
        handle.repaint_full(&mut commands, entity);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_add_injects_render_bundle() {
        use bevy::asset::AssetPlugin;

        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()));
        app.init_resource::<Assets<TerminalUiMaterial>>();
        app.add_observer(on_add_inject_render);
        let entity = app.world_mut().spawn(OrzmaTerminal).id();
        app.update();
        assert!(
            app.world().entity(entity).contains::<TerminalGrid>(),
            "On<Add, OrzmaTerminal> must inject TerminalRenderBundle (TerminalGrid)",
        );
    }

    #[test]
    fn late_grid_add_repaints_terminal_content() {
        use orzma_tty_renderer::prelude::TerminalGridPlugin;

        // Reproduces the split-pane-black race: a terminal whose handle already
        // advanced content but whose render `TerminalGrid` is injected LATE.
        // Without `on_add_grid_repaint` the grid stays empty (its first snapshot
        // was already dropped and deltas cannot rebuild it) and the pane is black.
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(TerminalGridPlugin)
            .add_observer(on_add_grid_repaint);

        let mut handle = TerminalHandle::detached(80, 24);
        handle.advance(b"hello\r\n");
        let entity = app.world_mut().spawn(handle).id();

        // Grid appears late — mirrors on_add_inject_render's deferred insert.
        app.world_mut()
            .entity_mut(entity)
            .insert(TerminalGrid::default());
        app.update();

        let grid = app.world().get::<TerminalGrid>(entity).unwrap();
        assert!(
            grid.cols > 0 && grid.rows > 0,
            "late-added grid must be repainted with the handle's dimensions"
        );
        let text: String = grid
            .cells
            .iter()
            .flatten()
            .map(|cell| cell.text.as_str())
            .collect();
        assert!(
            text.contains("hello"),
            "repaint must carry the handle's existing content; got {text:?}"
        );
    }
}
