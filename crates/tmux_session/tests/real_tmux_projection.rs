//! Gated end-to-end test: connect to a real tmux and verify the projection
//! model populates from the live notification stream.
//! Run with: `cargo test -p ozmux_tmux --test real_tmux_projection -- --ignored`.

use bevy::prelude::*;
use ozmux_tmux::{ConnectionState, ProjectionModel, TmuxConnection, TmuxSessionPlugin};
use std::time::{Duration, Instant};
use tmux_control::TmuxServer;

#[test]
#[ignore = "requires a real tmux binary and a controlling PTY"]
fn projection_populates_from_real_tmux() {
    let socket = format!("ozmux-phase1b-{}", std::process::id());
    let server = TmuxServer::new().socket_name(&socket);
    let client = server.new_session().expect("spawn tmux -CC new-session");

    let mut app = App::new();
    app.add_plugins(TmuxSessionPlugin);
    app.world_mut()
        .get_non_send_resource_mut::<TmuxConnection>()
        .expect("TmuxConnection inserted by the plugin")
        .set(client);

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut has_window = false;
    while Instant::now() < deadline {
        app.update();
        if !app.world().resource::<ProjectionModel>().windows.is_empty() {
            has_window = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    assert_eq!(
        *app.world().resource::<ConnectionState>(),
        ConnectionState::Attached
    );
    assert!(
        has_window,
        "the projection should gain at least one window from the attach notifications"
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
