//! Gated end-to-end test against a real tmux `-CC`: a foreign client focusing a
//! pane in ANOTHER session must not spawn a phantom window in ozmux's projection.
//!
//! tmux's `control_notify_window_pane_changed` has no session guard — it
//! broadcasts `%window-pane-changed @win %pane` to every control client on the
//! server. So ozmux (attached to session A) receives a foreign session B's pane
//! focus. Without the fix, `on_active_pane_changed` would `ensure_window` the
//! foreign window id, spawning a `TmuxWindow { index: 0, name: "" }` (a "0:" tab)
//! and moving the `ActiveWindow` marker onto it — hiding ozmux's real window and
//! blanking the pane. The fix gates the live-notification path on `index.windows`
//! membership, so a foreign window is ignored.
//!
//! Run with:
//! ```text
//! cargo test -p ozmux_tmux --test real_tmux_pane_focus -- --ignored --test-threads=1
//! ```

use bevy::prelude::*;
use ozmux_tmux::{
    ActiveWindow, ConnectionState, TmuxConnection, TmuxPane, TmuxSessionPlugin, TmuxWindow,
    WindowId,
};
use std::collections::HashSet;
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
    let socket = format!("ozmux-pf-{}-{}", std::process::id(), tag);
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

fn window_ids(app: &mut App) -> HashSet<WindowId> {
    app.world_mut()
        .query::<&TmuxWindow>()
        .iter(app.world())
        .map(|w| w.id)
        .collect()
}

fn active_window_id(app: &mut App) -> Option<WindowId> {
    app.world_mut()
        .query_filtered::<&TmuxWindow, With<ActiveWindow>>()
        .iter(app.world())
        .next()
        .map(|w| w.id)
}

#[test]
#[ignore = "requires a real tmux binary and a controlling PTY"]
fn foreign_pane_focus_does_not_spawn_phantom_window() {
    let mut app = attach_and_wait("panefocus");
    let foreign = format!("ozmux-pf-foreign-{}", std::process::id());

    let handle = app
        .world()
        .get_non_send_resource::<TmuxConnection>()
        .unwrap()
        .client()
        .unwrap()
        .handle();
    // Foreign session B with two panes in its window.
    handle
        .send(&format!("new-session -d -s {foreign}"))
        .expect("new-session -d");
    handle
        .send(&format!("split-window -t {foreign}:"))
        .expect("split-window");

    // Let the foreign-session setup notifications settle, then capture ozmux's own
    // window-id set (foreign windows are not projected) and active window.
    let _ = pump_until(&mut app, 2, |_| false);
    let baseline_ids = window_ids(&mut app);
    let active_before = active_window_id(&mut app);

    // Foreign client focuses the other pane in session B → tmux broadcasts
    // %window-pane-changed to ozmux's control client.
    handle
        .send(&format!("select-pane -t {foreign}:.+"))
        .expect("select-pane");

    let _ = pump_until(&mut app, 2, |_| false);

    let post_ids = window_ids(&mut app);
    let active_after = active_window_id(&mut app);
    teardown(&mut app);

    assert_eq!(
        post_ids, baseline_ids,
        "a foreign session's pane focus must not add a window to ozmux's projection \
         (no phantom \"0:\" tab); baseline={baseline_ids:?} after={post_ids:?}"
    );
    assert_eq!(
        active_after, active_before,
        "a foreign session's pane focus must not move ozmux's ActiveWindow marker; \
         before={active_before:?} after={active_after:?}"
    );
}
