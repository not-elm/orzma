//! Gated end-to-end test: a session with two windows attaches and the
//! projection populates both windows WITH panes — exercising the
//! list-windows enumeration + seed path.
//! Run with: `cargo test -p ozmux_tmux --test real_tmux_enumeration -- --ignored`.

use bevy::prelude::*;
use ozmux_tmux::{ProjectionModel, TmuxConnection, TmuxSessionPlugin};
use std::time::{Duration, Instant};
use tmux_control::TmuxServer;

#[test]
#[ignore = "requires a real tmux binary and a controlling PTY"]
fn enumeration_populates_existing_windows_with_panes() {
    let socket = format!("ozmux-phase1c-{}", std::process::id());
    let server = TmuxServer::new().socket_name(&socket);
    let client = server.new_session().expect("spawn tmux -CC new-session");

    client.handle().send("new-window").expect("new-window");
    std::thread::sleep(Duration::from_millis(500));

    let mut app = App::new();
    app.add_plugins(TmuxSessionPlugin);
    app.world_mut()
        .get_non_send_resource_mut::<TmuxConnection>()
        .expect("TmuxConnection inserted by the plugin")
        .set(client);

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut ready = false;
    while Instant::now() < deadline {
        app.update();
        let model = app.world().resource::<ProjectionModel>();
        if model.windows.len() >= 2 && model.windows.iter().all(|w| !w.panes.is_empty()) {
            ready = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    assert!(
        ready,
        "both windows should be projected with panes (via the list-windows seed)"
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
