//! Gated end-to-end test: attach a real tmux `-CC` client, await indexed
//! windows and a populated session name, create a second window via
//! `new-window`, then switch back to the original with `select-window` and
//! assert the active flag flips — verifying the command-echo path.
//! Run with: `cargo test -p ozmux_tmux --test real_tmux_window -- --ignored`.

use bevy::prelude::*;
use ozmux_tmux::{
    ActiveWindow, ConnectionState, TmuxConnection, TmuxSession, TmuxSessionPlugin, TmuxWindow,
    WindowId, select_window_command,
};
use std::time::{Duration, Instant};
use tmux_control::TmuxServer;

fn has_session_name(world: &mut World) -> bool {
    world
        .query::<&TmuxSession>()
        .iter(world)
        .any(|session| !session.name.is_empty())
}

fn window_count(world: &mut World) -> usize {
    world.query::<&TmuxWindow>().iter(world).count()
}

fn window_indices(world: &mut World) -> std::collections::HashSet<u32> {
    world
        .query::<&TmuxWindow>()
        .iter(world)
        .map(|window| window.index)
        .collect()
}

fn active_window_id(world: &mut World) -> Option<WindowId> {
    world
        .query_filtered::<&TmuxWindow, With<ActiveWindow>>()
        .iter(world)
        .next()
        .map(|window| window.id)
}

#[test]
#[ignore = "requires a real tmux binary and a controlling PTY"]
fn window_switch_round_trips_via_select_window() {
    let socket = format!("ozmux-phase3b-{}", std::process::id());
    let server = TmuxServer::new().socket_name(&socket);
    let client = server.new_session().expect("spawn tmux -CC new-session");

    let mut app = App::new();
    app.add_plugins(TmuxSessionPlugin);
    app.world_mut()
        .get_non_send_resource_mut::<TmuxConnection>()
        .expect("TmuxConnection inserted by the plugin")
        .set(client);

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        app.update();
        let attached = *app.world().resource::<ConnectionState>() == ConnectionState::Attached;
        let named = has_session_name(app.world_mut());
        let has_windows = window_count(app.world_mut()) > 0;
        if attached && named && has_windows {
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
        has_session_name(app.world_mut()),
        "the session name should be populated from %%session-changed"
    );
    assert!(
        window_count(app.world_mut()) > 0,
        "at least one window should be projected"
    );

    let first_active: WindowId =
        active_window_id(app.world_mut()).expect("one window should be active");

    app.world()
        .get_non_send_resource::<TmuxConnection>()
        .unwrap()
        .client()
        .unwrap()
        .handle()
        .send("new-window")
        .expect("new-window");

    let new_win_deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < new_win_deadline {
        app.update();
        if window_count(app.world_mut()) >= 2 {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    assert!(
        window_count(app.world_mut()) >= 2,
        "a second window should appear after new-window"
    );

    let indices = window_indices(app.world_mut());
    assert_eq!(
        indices.len(),
        window_count(app.world_mut()),
        "each window must carry a distinct tmux index",
    );

    let depart_deadline = Instant::now() + Duration::from_secs(5);
    let mut departed = false;
    while Instant::now() < depart_deadline {
        app.update();
        if active_window_id(app.world_mut()) != Some(first_active) {
            departed = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        departed,
        "new-window should shift the active window off first_active"
    );

    let cmd = select_window_command(first_active);
    app.world()
        .get_non_send_resource::<TmuxConnection>()
        .unwrap()
        .client()
        .unwrap()
        .handle()
        .send(&cmd)
        .expect("select-window");

    let switch_deadline = Instant::now() + Duration::from_secs(5);
    let mut switched = false;
    while Instant::now() < switch_deadline {
        app.update();
        if active_window_id(app.world_mut()) == Some(first_active) {
            switched = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    assert!(
        switched,
        "active window should flip back to {:?} after select-window",
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
