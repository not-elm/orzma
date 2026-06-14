//! End-to-end test against a real tmux binary. Gated with `#[ignore]`
//! because it requires `tmux` on `PATH` and spawns a server on a private
//! socket. Run with: `cargo test -p ozmux_tmux --test real_tmux -- --ignored`.

use bevy::prelude::*;
use ozmux_tmux::{ConnectionState, TmuxConnection, TmuxSessionPlugin};
use std::time::{Duration, Instant};
use tmux_control::TmuxServer;

#[test]
#[ignore = "requires a real tmux binary and a controlling PTY"]
fn attaches_and_drains_events_from_real_tmux() {
    let socket = format!("ozmux-phase0-{}", std::process::id());
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
        if *app.world().resource::<ConnectionState>() == ConnectionState::Attached {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    assert_eq!(
        *app.world().resource::<ConnectionState>(),
        ConnectionState::Attached,
        "should reach Attached after draining real tmux events"
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
