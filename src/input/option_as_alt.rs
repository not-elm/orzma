//! macOS "Option as Meta" support: applies the `[keyboard] option_as_alt`
//! config to the native winit window via `WindowExtMacOS::set_option_as_alt`,
//! so the configured Option side is delivered as Alt (Meta) below the IME
//! layer instead of composing into special characters. No-op on non-macOS.

use bevy::prelude::*;

/// Bevy plugin that applies the configured macOS Option-as-Alt mode to the
/// primary window. Empty on non-macOS targets.
pub(super) struct OptionAsAltPlugin;

impl Plugin for OptionAsAltPlugin {
    fn build(&self, app: &mut App) {
        #[cfg(target_os = "macos")]
        app.add_systems(Update, macos::apply_option_as_alt);
        #[cfg(not(target_os = "macos"))]
        let _ = app;
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use crate::configs::OrzmaConfigsResource;
    use bevy::ecs::system::NonSendMarker;
    use bevy::prelude::*;
    use bevy::window::PrimaryWindow;
    use bevy::winit::WINIT_WINDOWS;
    use orzma_configs::keyboard::OptionAsAlt;
    use winit::platform::macos::{OptionAsAlt as WinitOptionAsAlt, WindowExtMacOS};

    pub(super) fn apply_option_as_alt(
        mut done: Local<bool>,
        configs: Res<OrzmaConfigsResource>,
        primary: Query<Entity, With<PrimaryWindow>>,
        _non_send_marker: NonSendMarker,
    ) {
        if *done {
            return;
        }
        let Ok(entity) = primary.single() else {
            return;
        };
        // NOTE: Bevy 0.18 keeps WinitWindows in a main-thread thread-local, not a world
        // non-send resource (bevy ECS #17667 workaround). NonSendMarker pins this system
        // to the main thread so the borrow sees the populated instance, not an empty one.
        WINIT_WINDOWS.with_borrow(|winit_windows| {
            if let Some(window) = winit_windows.get_window(entity) {
                window.set_option_as_alt(to_winit(configs.keyboard.option_as_alt));
                *done = true;
            }
        });
    }

    fn to_winit(mode: OptionAsAlt) -> WinitOptionAsAlt {
        match mode {
            OptionAsAlt::None => WinitOptionAsAlt::None,
            OptionAsAlt::Left => WinitOptionAsAlt::OnlyLeft,
            OptionAsAlt::Right => WinitOptionAsAlt::OnlyRight,
            OptionAsAlt::Both => WinitOptionAsAlt::Both,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn maps_each_variant_to_winit() {
            assert!(matches!(
                to_winit(OptionAsAlt::None),
                WinitOptionAsAlt::None
            ));
            assert!(matches!(
                to_winit(OptionAsAlt::Left),
                WinitOptionAsAlt::OnlyLeft
            ));
            assert!(matches!(
                to_winit(OptionAsAlt::Right),
                WinitOptionAsAlt::OnlyRight
            ));
            assert!(matches!(
                to_winit(OptionAsAlt::Both),
                WinitOptionAsAlt::Both
            ));
        }

        #[test]
        fn system_validates_and_runs_without_winit() {
            let mut app = App::new();
            app.add_plugins(MinimalPlugins);
            app.init_resource::<crate::configs::OrzmaConfigsResource>();
            app.world_mut().spawn(PrimaryWindow);
            app.add_systems(Update, apply_option_as_alt);
            app.update();
        }
    }
}
