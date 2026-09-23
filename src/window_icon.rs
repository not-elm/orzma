//! Windows title-bar and taskbar icon: applies the orzma icon that
//! `build.rs` embeds in the executable to the primary window. No-op on
//! non-Windows targets.

use bevy::prelude::*;

/// Sets the primary window's title-bar and taskbar icon to the orzma icon
/// embedded in the executable. Empty on non-Windows targets.
pub(crate) struct WindowIconPlugin;

impl Plugin for WindowIconPlugin {
    fn build(&self, app: &mut App) {
        #[cfg(windows)]
        app.add_systems(Update, windows::apply_window_icon);
        #[cfg(not(windows))]
        let _ = app;
    }
}

#[cfg(windows)]
mod windows {
    use bevy::ecs::system::NonSendMarker;
    use bevy::prelude::*;
    use bevy::window::PrimaryWindow;
    use bevy::winit::WINIT_WINDOWS;
    use winit::dpi::PhysicalSize;
    use winit::platform::windows::{IconExtWindows, WindowExtWindows};
    use winit::window::Icon;

    /// Resource ordinal `build/windows/orzma.rc` assigns to the icon.
    const ICON_ORDINAL: u16 = 1;
    /// Largest frame in `orzma.ico`, loaded for the taskbar's `ICON_BIG`.
    const TASKBAR_ICON_SIZE: u32 = 256;

    pub(super) fn apply_window_icon(
        mut done: Local<bool>,
        primary: Query<Entity, With<PrimaryWindow>>,
        _non_send_marker: NonSendMarker,
    ) {
        if *done {
            return;
        }
        let Ok(entity) = primary.single() else {
            return;
        };
        // NOTE: Bevy 0.19 keeps WinitWindows in a main-thread thread-local, not a world
        // non-send resource (bevy ECS #17667 workaround). NonSendMarker pins this system
        // to the main thread so the borrow sees the populated instance, not an empty one.
        WINIT_WINDOWS.with_borrow(|winit_windows| {
            let Some(window) = winit_windows.get_window(entity) else {
                return;
            };
            *done = true;
            let taskbar_size = PhysicalSize::new(TASKBAR_ICON_SIZE, TASKBAR_ICON_SIZE);
            match (
                Icon::from_resource(ICON_ORDINAL, None),
                Icon::from_resource(ICON_ORDINAL, Some(taskbar_size)),
            ) {
                (Ok(window_icon), Ok(taskbar_icon)) => {
                    window.set_window_icon(Some(window_icon));
                    window.set_taskbar_icon(Some(taskbar_icon));
                }
                (Err(err), _) | (_, Err(err)) => {
                    warn!("failed to load the embedded orzma window icon: {err}");
                }
            }
        });
    }
}
