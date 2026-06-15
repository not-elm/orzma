//! Gated end-to-end test: attach a real tmux `-CC` client, split the initial
//! window to get two panes, then use `select-pane` to return focus to the
//! first pane and assert `active_pane` flips back — verifying the
//! command-echo path for pane focus.
//! Run with: `cargo test -p ozmux_tmux --test real_tmux_pane -- --ignored`.

use bevy::prelude::*;
use ozmux_tmux::{
    ConnectionState, PaneId, ProjectionModel, TmuxConnection, TmuxSessionPlugin,
    select_pane_command,
};
use std::time::{Duration, Instant};
use tmux_control::TmuxServer;

#[test]
#[ignore = "requires a real tmux binary and a controlling PTY"]
fn select_pane_round_trips_active_pane() {
    let socket = format!("ozmux-phase3c-{}", std::process::id());
    let server = TmuxServer::new().socket_name(&socket);
    let client = server.new_session().expect("spawn tmux -CC new-session");

    let mut app = App::new();
    app.add_plugins(TmuxSessionPlugin);
    app.world_mut()
        .get_non_send_resource_mut::<TmuxConnection>()
        .expect("TmuxConnection inserted by the plugin")
        .set(client);

    let attach_deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < attach_deadline {
        app.update();
        let attached = *app.world().resource::<ConnectionState>() == ConnectionState::Attached;
        let has_active_pane = app
            .world()
            .resource::<ProjectionModel>()
            .active_pane
            .is_some();
        if attached && has_active_pane {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    assert_eq!(
        *app.world().resource::<ConnectionState>(),
        ConnectionState::Attached,
        "should reach Attached within 5 s"
    );
    assert!(
        app.world()
            .resource::<ProjectionModel>()
            .active_pane
            .is_some(),
        "active_pane should be populated from %%window-pane-changed"
    );

    let first_active: PaneId = app
        .world()
        .resource::<ProjectionModel>()
        .active_pane
        .expect("active_pane is some");

    app.world()
        .get_non_send_resource::<TmuxConnection>()
        .unwrap()
        .client()
        .unwrap()
        .handle()
        .send("split-window")
        .expect("split-window");

    let split_deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < split_deadline {
        app.update();
        let pane_count = app
            .world()
            .resource::<ProjectionModel>()
            .windows
            .iter()
            .find(|w| w.active)
            .map(|w| w.panes.len())
            .unwrap_or(0);
        if pane_count >= 2 {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    assert!(
        app.world()
            .resource::<ProjectionModel>()
            .windows
            .iter()
            .find(|w| w.active)
            .map(|w| w.panes.len())
            .unwrap_or(0)
            >= 2,
        "at least 2 panes should appear in the active window after split-window"
    );

    let depart_deadline = Instant::now() + Duration::from_secs(5);
    let mut departed = false;
    while Instant::now() < depart_deadline {
        app.update();
        let active_pane = app.world().resource::<ProjectionModel>().active_pane;
        if active_pane != Some(first_active) {
            departed = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        departed,
        "split-window should shift the active pane off first_active"
    );

    let cmd = select_pane_command(first_active);
    app.world()
        .get_non_send_resource::<TmuxConnection>()
        .unwrap()
        .client()
        .unwrap()
        .handle()
        .send(&cmd)
        .expect("select-pane");

    let switch_deadline = Instant::now() + Duration::from_secs(5);
    let mut switched = false;
    while Instant::now() < switch_deadline {
        app.update();
        let active_pane = app.world().resource::<ProjectionModel>().active_pane;
        if active_pane == Some(first_active) {
            switched = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    assert!(
        switched,
        "select-pane should return focus to first_active {:?}",
        first_active
    );

    if let Some(client) = app
        .world_mut()
        .get_non_send_resource_mut::<TmuxConnection>()
        .expect("TmuxConnection present")
        .take()
    {
        client.handle().send("kill-server").ok();
    }
}
