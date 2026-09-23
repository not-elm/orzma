//! Sets how often the app updates without outside input: on demand, or at
//! a steady tick while a webview needs CEF pumped.

use bevy::prelude::*;
use bevy::winit::{UpdateMode, WinitSettings};
use bevy_cef::prelude::{BeginFrameInterval, WebviewSource};
use std::time::Duration;

/// Keeps `WinitSettings` on the update policy the current webviews call
/// for.
pub(super) struct UpdatePolicyPlugin;

impl Plugin for UpdatePolicyPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(UpdatePolicy::OnDemand.winit_settings())
            .insert_resource(BeginFrameInterval(BEGIN_FRAME_INTERVAL))
            .add_systems(Last, apply_update_policy);
    }
}

const ON_DEMAND_FOCUSED_WAIT: Duration = Duration::from_secs(5);
const ON_DEMAND_UNFOCUSED_WAIT: Duration = Duration::from_secs(60);
const WEBVIEW_TICK: Duration = Duration::from_millis(1000 / 30);
const WEBVIEW_GRACE: Duration = Duration::from_secs(2);
const BEGIN_FRAME_INTERVAL: Duration = Duration::from_millis(30);

/// How often the app updates without outside input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UpdatePolicy {
    /// Updates only when woken, with a safety tick of 5 s while focused
    /// and 60 s while unfocused.
    OnDemand,
    /// Updates about 30 times a second.
    WebviewTick,
}

impl UpdatePolicy {
    /// `WebviewTick` while a webview exists and for `WEBVIEW_GRACE` after
    /// the last one went away; otherwise `OnDemand`.
    fn decide(webviews_present: bool, since_last_webview: Option<Duration>) -> Self {
        let recent = since_last_webview.is_some_and(|since| since < WEBVIEW_GRACE);
        if webviews_present || recent {
            Self::WebviewTick
        } else {
            Self::OnDemand
        }
    }

    /// The winit settings this policy runs the app with.
    fn winit_settings(self) -> WinitSettings {
        let (focused, unfocused) = match self {
            Self::OnDemand => (ON_DEMAND_FOCUSED_WAIT, ON_DEMAND_UNFOCUSED_WAIT),
            Self::WebviewTick => (WEBVIEW_TICK, WEBVIEW_TICK),
        };
        WinitSettings {
            focused_mode: UpdateMode::reactive_low_power(focused),
            unfocused_mode: UpdateMode::reactive_low_power(unfocused),
        }
    }
}

/// Switches `WinitSettings` to the policy the current webviews call for,
/// writing only when the policy changes.
fn apply_update_policy(
    mut settings: ResMut<WinitSettings>,
    mut last_webview: Local<Option<Duration>>,
    webviews: Query<(), With<WebviewSource>>,
    time: Res<Time<Real>>,
) {
    let now = time.elapsed();
    let present = !webviews.is_empty();
    if present {
        *last_webview = Some(now);
    }
    let since = last_webview.map(|seen| now.saturating_sub(seen));
    let wanted = UpdatePolicy::decide(present, since).winit_settings();
    if settings.focused_mode != wanted.focused_mode
        || settings.unfocused_mode != wanted.unfocused_mode
    {
        *settings = wanted;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::time::TimeUpdateStrategy;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
                100,
            )))
            .add_plugins(UpdatePolicyPlugin);
        app
    }

    fn modes(app: &App) -> (UpdateMode, UpdateMode) {
        let settings = app.world().resource::<WinitSettings>();
        (settings.focused_mode, settings.unfocused_mode)
    }

    fn policy_modes(policy: UpdatePolicy) -> (UpdateMode, UpdateMode) {
        let settings = policy.winit_settings();
        (settings.focused_mode, settings.unfocused_mode)
    }

    fn webview() -> WebviewSource {
        WebviewSource::inline("<p>page</p>")
    }

    /// Asserts that each policy maps to its low-power reactive modes.
    ///
    /// Case: orzma idles as a plain terminal, and separately shows orzmd's
    /// rendered page.
    #[test]
    fn each_policy_maps_to_its_low_power_modes() {
        assert_eq!(
            policy_modes(UpdatePolicy::OnDemand),
            (
                UpdateMode::reactive_low_power(Duration::from_secs(5)),
                UpdateMode::reactive_low_power(Duration::from_secs(60)),
            )
        );
        assert_eq!(
            policy_modes(UpdatePolicy::WebviewTick),
            (
                UpdateMode::reactive_low_power(Duration::from_millis(33)),
                UpdateMode::reactive_low_power(Duration::from_millis(33)),
            )
        );
    }

    /// Asserts that the tick holds while a webview exists and for the grace
    /// period after the last one went away.
    ///
    /// Case: orzmd mounts a page, and later the user closes it.
    #[test]
    fn the_policy_ticks_while_a_webview_exists_and_through_the_grace() {
        assert_eq!(UpdatePolicy::decide(false, None), UpdatePolicy::OnDemand);
        assert_eq!(UpdatePolicy::decide(true, None), UpdatePolicy::WebviewTick);
        assert_eq!(
            UpdatePolicy::decide(false, Some(Duration::from_millis(1999))),
            UpdatePolicy::WebviewTick
        );
        assert_eq!(
            UpdatePolicy::decide(false, Some(Duration::from_secs(2))),
            UpdatePolicy::OnDemand
        );
    }

    /// Asserts that an idle frame leaves `WinitSettings` unwritten, and that
    /// the app ticks from the frame a webview appears until the grace period
    /// after it goes away, then runs on demand.
    ///
    /// Case: orzmd mounts a page while orzma is idle, and the user closes
    /// it a while later.
    #[test]
    fn the_tick_starts_with_a_webview_and_stops_after_the_grace() {
        let mut app = app();
        app.update();
        assert_eq!(modes(&app), policy_modes(UpdatePolicy::OnDemand));
        let idle = app.world().resource_ref::<WinitSettings>().last_changed();
        app.update();
        assert_eq!(
            app.world().resource_ref::<WinitSettings>().last_changed(),
            idle
        );

        let page = app.world_mut().spawn(webview()).id();
        app.update();
        assert_eq!(modes(&app), policy_modes(UpdatePolicy::WebviewTick));

        assert!(app.world_mut().despawn(page));
        app.update();
        assert_eq!(modes(&app), policy_modes(UpdatePolicy::WebviewTick));

        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs(3)));
        app.update();
        assert_eq!(modes(&app), policy_modes(UpdatePolicy::OnDemand));
    }

    /// Asserts that a webview remounted within the grace period keeps the
    /// tick without rewriting `WinitSettings`.
    ///
    /// Case: a program unmounts its page and mounts the next one a moment
    /// later.
    #[test]
    fn a_remount_within_the_grace_keeps_the_tick_without_a_rewrite() {
        let mut app = app();
        let page = app.world_mut().spawn(webview()).id();
        app.update();
        let ticking = app.world().resource_ref::<WinitSettings>().last_changed();

        assert!(app.world_mut().despawn(page));
        app.update();
        app.world_mut().spawn(webview());
        app.update();
        assert_eq!(modes(&app), policy_modes(UpdatePolicy::WebviewTick));
        assert_eq!(
            app.world().resource_ref::<WinitSettings>().last_changed(),
            ticking
        );
    }

    /// Asserts that the plugin lowers CEF's begin-frame interval below the
    /// tick.
    ///
    /// Case: a page animates on macOS, where CEF paints only on begin frames.
    #[test]
    fn the_begin_frame_interval_is_below_the_tick() {
        let app = app();
        assert_eq!(
            app.world().resource::<BeginFrameInterval>().0,
            Duration::from_millis(30)
        );
    }
}
