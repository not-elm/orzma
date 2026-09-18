//! Renderer failure policy: a GPU error stops drawing, or rebuilds the
//! render device, rather than ending the session.

use bevy::prelude::*;
use bevy::render::error_handler::{ErrorType, RenderError, RenderErrorHandler, RenderErrorPolicy};
use bevy::render::settings::RenderCreation;
use bevy::window::WindowOccluded;
use std::time::Duration;

/// The shortest interval between two render-device rebuilds.
const REBUILD_RETRY_INTERVAL: Duration = Duration::from_secs(1);

/// Installs the renderer error policy: a renderer error stops drawing or
/// rebuilds the render device, and never ends the session.
pub(crate) struct RenderErrorPlugin;

impl Plugin for RenderErrorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RenderRecovery>()
            .insert_resource(RenderErrorHandler(on_render_error))
            .add_systems(Update, track_occlusion.run_if(on_message::<WindowOccluded>));
    }
}

/// What the renderer does about an error that just fired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RenderRecoveryAction {
    /// Rebuild the render device and resume drawing.
    Rebuild,
    /// Stop drawing, for the first time since the last rebuild.
    StopAndReport,
    /// Stop drawing, having already reported it.
    Stop,
}

/// Rebuild history: when the render device was last rebuilt, whether the
/// current stop has been reported, and whether the window is hidden.
#[derive(Resource, Default)]
struct RenderRecovery {
    last_rebuild: Option<Duration>,
    reported: bool,
    occluded: bool,
}

/// Mirrors the window's occlusion into [`RenderRecovery`].
fn track_occlusion(
    mut recovery: ResMut<RenderRecovery>,
    mut occlusions: MessageReader<WindowOccluded>,
) {
    let Some(latest) = occlusions.read().last() else {
        return;
    };
    if recovery.occluded != latest.occluded {
        recovery.occluded = latest.occluded;
    }
}

/// Advances the recovery state for one renderer error and returns what to
/// do about it. `now` is the caller's `Time<Real>::elapsed()`.
///
/// A lost device is rebuilt at most once per [`REBUILD_RETRY_INTERVAL`],
/// and never while the window is occluded; every other error type stops
/// drawing rather than rebuilding, and a stop is reported once until the
/// next rebuild.
fn step_recovery(
    recovery: &mut RenderRecovery,
    ty: ErrorType,
    now: Duration,
) -> RenderRecoveryAction {
    if ty == ErrorType::DeviceLost {
        if recovery.occluded {
            return RenderRecoveryAction::Stop;
        }
        let due = recovery
            .last_rebuild
            .is_none_or(|last| now.saturating_sub(last) >= REBUILD_RETRY_INTERVAL);
        if !due {
            return RenderRecoveryAction::Stop;
        }
        recovery.last_rebuild = Some(now);
        recovery.reported = false;
        return RenderRecoveryAction::Rebuild;
    }
    if recovery.reported {
        return RenderRecoveryAction::Stop;
    }
    recovery.reported = true;
    RenderRecoveryAction::StopAndReport
}

/// Decides the policy for one renderer error, keeping the session alive
/// whatever the error is.
fn on_render_error(
    error: &RenderError,
    main_world: &mut World,
    _render_world: &mut World,
) -> RenderErrorPolicy {
    let now = main_world
        .get_resource::<Time<Real>>()
        .map_or(Duration::ZERO, |time| time.elapsed());
    let Some(mut recovery) = main_world.get_resource_mut::<RenderRecovery>() else {
        return RenderErrorPolicy::StopRendering;
    };
    match step_recovery(&mut recovery, error.ty, now) {
        RenderRecoveryAction::Rebuild => {
            warn!(
                "rebuilding the renderer after a {:?} error: {}",
                error.ty, error.description
            );
            RenderErrorPolicy::Recover(RenderCreation::Automatic(Box::default()))
        }
        RenderRecoveryAction::StopAndReport => {
            error!(
                "stopped drawing after a {:?} error; the session stays alive: {}",
                error.ty, error.description
            );
            RenderErrorPolicy::StopRendering
        }
        RenderRecoveryAction::Stop => RenderErrorPolicy::StopRendering,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device_lost() -> RenderError {
        RenderError {
            ty: ErrorType::DeviceLost,
            description: "device lost".to_string(),
            source: None,
        }
    }

    /// Asserts that the first lost device rebuilds the renderer rather than
    /// waiting for the retry interval to pass.
    ///
    /// Case: the display turns off while the terminal is running, and macOS
    /// takes the Metal device away for the first time in this session.
    #[test]
    fn a_first_lost_device_is_rebuilt_immediately() {
        let mut recovery = RenderRecovery::default();
        assert_eq!(
            step_recovery(&mut recovery, ErrorType::DeviceLost, Duration::ZERO),
            RenderRecoveryAction::Rebuild,
        );
    }

    /// Asserts that a second lost device inside the retry interval stops
    /// drawing rather than rebuilding again.
    ///
    /// Case: the display stays off, so every frame after the first rebuild
    /// reports the same lost device.
    #[test]
    fn a_lost_device_inside_the_retry_interval_stops() {
        let mut recovery = RenderRecovery::default();
        step_recovery(&mut recovery, ErrorType::DeviceLost, Duration::ZERO);
        assert_eq!(
            step_recovery(
                &mut recovery,
                ErrorType::DeviceLost,
                REBUILD_RETRY_INTERVAL - Duration::from_millis(1),
            ),
            RenderRecoveryAction::Stop,
        );
    }

    /// Asserts that a lost device is rebuilt again once the retry interval
    /// has passed.
    ///
    /// Case: the user wakes the display minutes later and the terminal has
    /// to acquire a working device again.
    #[test]
    fn a_lost_device_after_the_retry_interval_is_rebuilt_again() {
        let mut recovery = RenderRecovery::default();
        step_recovery(&mut recovery, ErrorType::DeviceLost, Duration::ZERO);
        assert_eq!(
            step_recovery(&mut recovery, ErrorType::DeviceLost, REBUILD_RETRY_INTERVAL),
            RenderRecoveryAction::Rebuild,
        );
    }

    /// Asserts that an error other than a lost device stops drawing rather
    /// than rebuilding the device.
    ///
    /// Case: a shader or draw call the terminal renderer issues trips wgpu's
    /// validation layer, which a new device would not fix.
    #[test]
    fn a_validation_error_stops_without_rebuilding() {
        let mut recovery = RenderRecovery::default();
        assert_eq!(
            step_recovery(&mut recovery, ErrorType::Validation, Duration::ZERO),
            RenderRecoveryAction::StopAndReport,
        );
        assert_eq!(recovery.last_rebuild, None);
    }

    /// Asserts that a stop is reported once rather than on every frame it
    /// persists.
    ///
    /// Case: a validation error keeps firing while the renderer sits
    /// stopped, and the handler runs again on each of those frames.
    #[test]
    fn a_persisting_stop_is_reported_once() {
        let mut recovery = RenderRecovery::default();
        step_recovery(&mut recovery, ErrorType::Validation, Duration::ZERO);
        assert_eq!(
            step_recovery(&mut recovery, ErrorType::Validation, Duration::from_secs(9)),
            RenderRecoveryAction::Stop,
        );
    }

    /// Asserts that a stop following a rebuild is reported again.
    ///
    /// Case: the terminal recovers from a lost device, then hits an
    /// unrelated error later in the same session.
    #[test]
    fn a_stop_after_a_rebuild_is_reported_again() {
        let mut recovery = RenderRecovery::default();
        step_recovery(&mut recovery, ErrorType::Validation, Duration::ZERO);
        step_recovery(&mut recovery, ErrorType::DeviceLost, Duration::from_secs(1));
        assert_eq!(
            step_recovery(&mut recovery, ErrorType::Internal, Duration::from_secs(2)),
            RenderRecoveryAction::StopAndReport,
        );
    }

    /// Asserts that a lost device is not rebuilt while the window is
    /// occluded, since the rebuild would run against a display that is off.
    ///
    /// Case: the laptop lid closes, macOS takes the device away, and the
    /// renderer keeps reporting the loss while the screen stays dark.
    #[test]
    fn a_lost_device_is_not_rebuilt_while_occluded() {
        let mut recovery = RenderRecovery {
            occluded: true,
            ..Default::default()
        };
        assert_eq!(
            step_recovery(&mut recovery, ErrorType::DeviceLost, Duration::ZERO),
            RenderRecoveryAction::Stop,
        );
        assert_eq!(recovery.last_rebuild, None);
    }

    /// Asserts that a lost device is rebuilt once the window is shown again.
    ///
    /// Case: the user opens the lid, the window stops being occluded, and the
    /// terminal has to draw again.
    #[test]
    fn a_lost_device_is_rebuilt_once_the_window_is_shown() {
        let mut recovery = RenderRecovery {
            occluded: true,
            ..Default::default()
        };
        step_recovery(&mut recovery, ErrorType::DeviceLost, Duration::ZERO);
        recovery.occluded = false;
        assert_eq!(
            step_recovery(
                &mut recovery,
                ErrorType::DeviceLost,
                Duration::from_millis(1)
            ),
            RenderRecoveryAction::Rebuild,
        );
    }

    /// Asserts that a `WindowOccluded` message is mirrored into the recovery
    /// state the policy reads.
    ///
    /// Case: macOS turns the display off and winit reports the window as
    /// occluded before the renderer sees anything wrong.
    #[test]
    fn an_occlusion_message_is_mirrored_into_the_recovery_state() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<WindowOccluded>()
            .init_resource::<RenderRecovery>()
            .add_systems(Update, track_occlusion.run_if(on_message::<WindowOccluded>));
        let window = app.world_mut().spawn_empty().id();
        app.world_mut().write_message(WindowOccluded {
            window,
            occluded: true,
        });
        app.update();

        assert!(
            app.world().resource::<RenderRecovery>().occluded,
            "an occluded window must be mirrored into RenderRecovery",
        );
    }

    /// Asserts that the installed handler keeps the session alive instead of
    /// writing `AppExit`.
    ///
    /// Case: the display turns off and Bevy hands the lost device to
    /// whichever error handler the app registered.
    #[test]
    fn the_installed_handler_never_ends_the_session() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<AppExit>()
            .add_message::<WindowOccluded>()
            .add_plugins(RenderErrorPlugin);
        app.update();

        let handler = app
            .world()
            .get_resource::<RenderErrorHandler>()
            .map(|handler| handler.0)
            .expect("RenderErrorPlugin must install a RenderErrorHandler");
        let mut render_world = World::new();
        handler(&device_lost(), app.world_mut(), &mut render_world);

        assert!(
            app.world()
                .resource::<Messages<AppExit>>()
                .iter_current_update_messages()
                .next()
                .is_none(),
            "a renderer error must not end the session",
        );
    }
}
