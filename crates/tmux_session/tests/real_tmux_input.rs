//! Gated end-to-end test: attach a real tmux `-CC` client, await the
//! client-name query, forward a keystroke via `send-keys -K -c <client>`,
//! and verify the typed character echoes back through `%output`.
//! Run with: `cargo test -p ozmux_tmux --test real_tmux_input -- --ignored`.

use bevy::prelude::*;
use ozmux_tmux::{
    ConnectionState, PaneOutput, TmuxConnection, TmuxSessionPlugin, send_keys_command,
};
use std::time::{Duration, Instant};
use tmux_control::TmuxServer;

#[derive(Resource, Default)]
struct Captured(Vec<u8>);

fn capture_output(mut sink: ResMut<Captured>, mut reader: MessageReader<PaneOutput>) {
    for msg in reader.read() {
        sink.0.extend_from_slice(&msg.data);
    }
}

#[test]
#[ignore = "requires a real tmux binary and a controlling PTY"]
fn send_keys_echoes_back_from_real_tmux() {
    let socket = format!("ozmux-phase3a-{}", std::process::id());
    let server = TmuxServer::new().socket_name(&socket);
    let client = server.new_session().expect("spawn tmux -CC new-session");

    let mut app = App::new();
    app.add_plugins(TmuxSessionPlugin);
    app.init_resource::<Captured>();
    app.add_systems(Update, capture_output);
    app.world_mut()
        .get_non_send_resource_mut::<TmuxConnection>()
        .expect("TmuxConnection inserted by the plugin")
        .set(client);

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut client_name: Option<String> = None;
    while Instant::now() < deadline {
        app.update();
        let attached = *app.world().resource::<ConnectionState>() == ConnectionState::Attached;
        if attached {
            client_name = app
                .world()
                .get_non_send_resource::<TmuxConnection>()
                .expect("TmuxConnection present")
                .client_name()
                .map(str::to_owned);
            if client_name.is_some() {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let client_name = client_name
        .expect("the display-message client-name query should complete shortly after attach");

    let cmd = send_keys_command(&client_name, &["a".to_string()]);
    assert!(cmd.starts_with("send-keys -K -c "));
    app.world()
        .get_non_send_resource::<TmuxConnection>()
        .unwrap()
        .client()
        .unwrap()
        .handle()
        .send(&cmd)
        .expect("send-keys -K");

    let echo_deadline = Instant::now() + Duration::from_secs(5);
    let mut echoed = false;
    while Instant::now() < echo_deadline {
        app.update();
        if app.world().resource::<Captured>().0.contains(&b'a') {
            echoed = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    assert!(echoed, "typed 'a' should echo back via %output");

    if let Some(client) = app
        .world_mut()
        .get_non_send_resource_mut::<TmuxConnection>()
        .expect("TmuxConnection present")
        .take()
    {
        client.handle().send("kill-server").ok();
    }
}
