//! Keeps windows alive when the monitor they sit on disappears, such as when
//! the display sleeps.

use bevy::prelude::*;
use bevy::window::{HasWindows, OnMonitor};
use std::mem;

/// Keeps every window alive across the removal of the monitor it was on.
///
/// A window keeps its [`OnMonitor`] while that monitor exists; when the
/// monitor is despawned, the window loses its [`OnMonitor`] instead of being
/// despawned along with the monitor.
pub(crate) struct WindowMonitorPlugin;

impl Plugin for WindowMonitorPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(detach_windows_from_lost_monitor);
    }
}

// TODO: remove this plugin once Bevy includes bevyengine/bevy#25427 (0.19.2 /
// 0.20), which stops despawning a monitor's windows along with the monitor.
fn detach_windows_from_lost_monitor(
    event: On<Despawn, HasWindows>,
    mut commands: Commands,
    mut monitors: Query<&mut HasWindows>,
) {
    let Ok(mut windows) = monitors.get_mut(event.entity) else {
        return;
    };
    // NOTE: the list must be emptied here, in place, from a `Despawn`
    // observer. Bevy runs `Despawn` observers before the `on_despawn` hook,
    // and that hook despawns every window `HasWindows` still lists, so a
    // queued removal or a later lifecycle event would come too late.
    let lost = mem::take(&mut *windows);
    for window in lost.iter() {
        commands.entity(window).try_remove::<OnMonitor>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::window::Monitor;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(WindowMonitorPlugin);
        app
    }

    fn window_on_a_monitor(app: &mut App) -> (Entity, Entity) {
        let world = app.world_mut();
        let monitor = world
            .spawn(Monitor {
                name: None,
                physical_height: 1080,
                physical_width: 1920,
                physical_position: IVec2::ZERO,
                refresh_rate_millihertz: None,
                scale_factor: 1.0,
                video_modes: Vec::new(),
            })
            .id();
        let window = world.spawn((Window::default(), OnMonitor(monitor))).id();
        (window, monitor)
    }

    /// Asserts that a window survives the despawn of the monitor it was
    /// placed on rather than being despawned along with it, and is left
    /// without an [`OnMonitor`].
    ///
    /// Case: the Mac's display turns off while orzma is open, and the window
    /// backend despawns the monitor the orzma window was on.
    #[test]
    fn window_survives_the_despawn_of_its_monitor() {
        let mut app = app();
        let (window, monitor) = window_on_a_monitor(&mut app);

        app.world_mut().despawn(monitor);

        let window = app
            .world()
            .get_entity(window)
            .expect("the window outlives its monitor");
        assert!(window.get::<OnMonitor>().is_none());
    }

    /// Asserts that a window keeps its [`OnMonitor`] while its monitor
    /// exists rather than having it removed.
    ///
    /// Case: orzma runs on a display that stays on, and the window backend
    /// has linked the orzma window to that display's monitor.
    #[test]
    fn window_keeps_its_monitor_while_the_monitor_exists() {
        let mut app = app();
        let (window, monitor) = window_on_a_monitor(&mut app);

        let link = app.world().get::<OnMonitor>(window).map(|link| link.0);
        assert_eq!(link, Some(monitor));
    }
}
