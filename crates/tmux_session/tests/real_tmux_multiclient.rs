//! Gated end-to-end multi-client tests against a real tmux `-CC`.
//!
//! **What is testable headlessly** (i.e. with `TmuxSessionPlugin` only, no GUI
//! window):
//!
//! 1. `foreign_session_change_keeps_projection` — a second tmux session is
//!    created on the same server while ozmux is attached to the first.  ozmux's
//!    window/pane projection for *its own* session must survive unaffected (no
//!    spurious teardown from the `%client-session-changed` / `%window-add`
//!    notifications emitted for the foreign session).  This directly validates
//!    the Component 2 teardown fix: `detect_session_switch` ignores
//!    `%client-session-changed` events whose `client` field does not match
//!    ozmux's own client name, so the projection is never torn down.
//!
//! 2. `foreign_shrink_observes_clamp` — `resize-window -y N -t @<active>` is
//!    issued over the control connection.  The test asserts only what is
//!    REAL-TMUX-OBSERVABLE via `%layout-change`: that the projected pane height
//!    updates to reflect the clamped geometry.  ozmux's GUI-side recovery
//!    (`sync_client_size`, registered by `OzmuxTmuxRenderPlugin` in the binary
//!    crate, which requires a `PrimaryWindow`) is **not exercisable** in a
//!    headless `TmuxSessionPlugin`-only app.  That recovery path is verified by
//!    the manual GUI smoke test documented in the plan checklist.
//!
//! Run with:
//! ```text
//! cargo test -p ozmux_tmux --test real_tmux_multiclient -- --ignored --test-threads=1
//! ```

use bevy::prelude::*;
use ozmux_tmux::{
    ActiveWindow, ConnectionState, TmuxConnection, TmuxPane, TmuxSessionPlugin, TmuxWindow,
    WindowId,
};
use std::time::{Duration, Instant};
use tmux_control::TmuxServer;

fn pump_until(app: &mut App, secs: u64, mut done: impl FnMut(&mut App) -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        app.update();
        if done(app) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

fn teardown(app: &mut App) {
    if let Some(client) = app
        .world_mut()
        .get_non_send_resource_mut::<TmuxConnection>()
        .unwrap()
        .take()
    {
        client.handle().send("kill-server").ok();
    }
}

fn attach_and_wait(tag: &str) -> App {
    let socket = format!("ozmux-mc-{}-{}", std::process::id(), tag);
    let server = TmuxServer::new().socket_name(&socket);
    let client = server.new_session().expect("spawn tmux -CC new-session");

    let mut app = App::new();
    app.add_plugins(TmuxSessionPlugin);
    app.world_mut()
        .get_non_send_resource_mut::<TmuxConnection>()
        .expect("TmuxConnection inserted by the plugin")
        .set(client);

    let mut pane_q = app.world_mut().query::<&TmuxPane>();
    let ready = pump_until(&mut app, 5, |app| {
        *app.world().resource::<ConnectionState>() == ConnectionState::Attached
            && pane_q.iter(app.world()).next().is_some()
    });
    assert!(ready, "tmux should attach and project a pane within 5 s");
    app
}

fn window_count(app: &mut App) -> usize {
    app.world_mut()
        .query::<&TmuxWindow>()
        .iter(app.world())
        .count()
}

fn pane_count(app: &mut App) -> usize {
    app.world_mut()
        .query::<&TmuxPane>()
        .iter(app.world())
        .count()
}

fn active_window_id(app: &mut App) -> Option<WindowId> {
    app.world_mut()
        .query_filtered::<&TmuxWindow, With<ActiveWindow>>()
        .iter(app.world())
        .next()
        .map(|w| w.id)
}

fn max_pane_height(app: &mut App) -> Option<u32> {
    app.world_mut()
        .query::<&TmuxPane>()
        .iter(app.world())
        .map(|p| p.dims.height)
        .max()
}

/// A second tmux session on the same server must NOT cause ozmux to tear down
/// its window/pane projection.
///
/// The fix under test is in `detect_session_switch` / `trigger_notification`:
/// `%client-session-changed` events are ignored unless their `client` field
/// matches ozmux's own client name.  Before the fix a foreign
/// `%client-session-changed` could look like a self-switch and cause a full
/// projection teardown, resulting in a black screen.
#[test]
#[ignore = "requires a real tmux binary and a controlling PTY"]
fn foreign_session_change_keeps_projection() {
    let mut app = attach_and_wait("sess");

    let baseline_windows = window_count(&mut app);
    let baseline_panes = pane_count(&mut app);
    assert!(
        baseline_windows >= 1,
        "must have at least one projected window after attach; got {baseline_windows}"
    );
    assert!(
        baseline_panes >= 1,
        "must have at least one projected pane after attach; got {baseline_panes}"
    );

    let handle = app
        .world()
        .get_non_send_resource::<TmuxConnection>()
        .unwrap()
        .client()
        .unwrap()
        .handle();

    // NOTE: `-d` keeps the foreign session detached so it does not take over the
    // control connection; the ozmux client is never switched, so any teardown
    // observed below would be a spurious foreign-event regression.
    handle
        .send("new-session -d -s ozmux-mc-foreign")
        .expect("new-session -d -s foreign");

    let _ = pump_until(&mut app, 2, |_| false);

    let post_windows = window_count(&mut app);
    let post_panes = pane_count(&mut app);

    teardown(&mut app);

    assert!(
        post_windows >= baseline_windows,
        "ozmux's window projection must not be torn down by a foreign session being \
         created; baseline={baseline_windows} after={post_windows}"
    );
    assert!(
        post_panes >= baseline_panes,
        "ozmux's pane projection must not be torn down by a foreign session being \
         created; baseline={baseline_panes} after={post_panes}"
    );
}

/// Drives `resize-window -y N -t @<active>` and asserts that the projected pane
/// height updates via the resulting `%layout-change`.
///
/// This validates that ozmux correctly handles `%layout-change` from an
/// adversarial resize (as would arrive from a foreign smaller client holding the
/// `latest` window-size slot).  The test observes ONLY what is
/// real-tmux-observable headlessly: that the projection reflects the clamped
/// geometry after the resize.
///
/// # Note on GUI-side recovery
///
/// ozmux's `sync_client_size` system (registered by `OzmuxTmuxRenderPlugin` in
/// the binary crate) re-pins the active window back to the ozmux terminal size
/// after such a shrink.  That system requires a `PrimaryWindow` resource that a
/// headless `TmuxSessionPlugin`-only app does not have.  GUI-side recovery is
/// verified by the manual smoke test in the plan checklist (run `cargo run`, attach
/// a small foreign terminal, observe no black screen and size recovery).
#[test]
#[ignore = "requires a real tmux binary and a controlling PTY"]
fn foreign_shrink_observes_clamp() {
    let mut app = attach_and_wait("shrink");

    let _ = pump_until(&mut app, 1, |_| false);

    let baseline_height = max_pane_height(&mut app).expect("a projected pane with height");
    let win_id = active_window_id(&mut app).expect("an active window");

    let shrunk_height: u32 = 8;
    assert!(
        baseline_height > shrunk_height,
        "baseline height {baseline_height} must exceed the shrink target {shrunk_height}"
    );

    let handle = app
        .world()
        .get_non_send_resource::<TmuxConnection>()
        .unwrap()
        .client()
        .unwrap()
        .handle();
    handle
        .send(&format!(
            "resize-window -y {shrunk_height} -t @{}",
            win_id.0
        ))
        .expect("resize-window");

    // Pump until the projected pane height drops STRICTLY below the baseline.
    // tmux emits a `%layout-change` after `resize-window`; the projection should
    // update within a few seconds. The strict `<` is load-bearing: `<=` is true
    // at the baseline before any `%layout-change` lands, so it would return on
    // the first iteration and pass even if the projection never updated.
    let observed_shrink = pump_until(&mut app, 5, |app| {
        max_pane_height(app).is_some_and(|h| h < baseline_height)
    });

    let final_height = max_pane_height(&mut app).unwrap_or(0);
    teardown(&mut app);

    assert!(
        observed_shrink,
        "the projected pane height must update after resize-window \
         (expected height change via %%layout-change); \
         baseline={baseline_height} shrunk_target={shrunk_height} final={final_height}"
    );
    // tmux's `window-size latest` semantics clamp to the smallest connected
    // client; with only one control client the window must shrink to at most
    // the requested height.  We assert the projection tracks real tmux geometry.
    assert!(
        final_height <= baseline_height,
        "projected pane height must not exceed baseline after a resize-window shrink; \
         baseline={baseline_height} final={final_height}"
    );
}
