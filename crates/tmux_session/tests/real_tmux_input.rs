//! Gated end-to-end tests against a real tmux `-CC`: forward keys to a pane via
//! `SendPaneKeys` (the production forward path) and observe the result
//! through `%output`.
//! Run with: `cargo test -p ozmux_tmux --test real_tmux_input -- --ignored`.

use bevy::prelude::*;
use ozmux_tmux::{
    ConnectionState, PaneOutput, SendPaneKeys, TmuxConnection, TmuxPane, TmuxSessionPlugin,
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

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn attach_and_project(tag: &str) -> (App, String) {
    let socket = format!("ozmux-key-{}-{}", std::process::id(), tag);
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

    let mut pane_q = app.world_mut().query::<&TmuxPane>();
    let ready = pump_until(&mut app, 5, |app| {
        *app.world().resource::<ConnectionState>() == ConnectionState::Attached
            && pane_q.iter(app.world()).next().is_some()
    });
    assert!(ready, "tmux should attach and project a pane within 5s");

    let target = pane_q
        .iter(app.world())
        .next()
        .map(|p| format!("%{}", p.id.0))
        .expect("a projected pane");
    (app, target)
}

fn handle_of(app: &App) -> tmux_control::TmuxHandle {
    app.world()
        .get_non_send_resource::<TmuxConnection>()
        .unwrap()
        .client()
        .unwrap()
        .handle()
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

#[test]
#[ignore = "requires a real tmux binary and a controlling PTY"]
fn key_a_echoes_back_from_real_tmux() {
    let (mut app, target) = attach_and_project("echo");
    let handle = handle_of(&app);
    handle
        .send(SendPaneKeys {
            pane: &target,
            names: &["a".to_string()],
        })
        .expect("send-keys a");
    let echoed = pump_until(&mut app, 5, |app| {
        app.world().resource::<Captured>().0.contains(&b'a')
    });
    teardown(&mut app);
    assert!(echoed, "typed 'a' should echo back via %output");
}

#[test]
#[ignore = "requires a real tmux binary and a controlling PTY"]
fn arrow_up_recalls_previous_command_via_pane_send() {
    let (mut app, target) = attach_and_project("arrowup");
    let handle = handle_of(&app);

    // Seed one shell-history entry (the pane runs an interactive shell, which
    // puts the terminal in application-cursor mode for readline).
    handle
        .send(&format!("send-keys -t {target} -l -- 'echo OZMUXHIST'"))
        .expect("type command");
    handle
        .send(&format!("send-keys -t {target} Enter"))
        .expect("run command");
    pump_until(&mut app, 2, |_| false);
    app.world_mut().resource_mut::<Captured>().0.clear();

    // Forward ArrowUp exactly as the production input plugin does.
    handle
        .send(SendPaneKeys {
            pane: &target,
            names: &["Up".to_string()],
        })
        .expect("forward Up");
    let recalled = pump_until(&mut app, 3, |app| {
        contains(&app.world().resource::<Captured>().0, b"echo OZMUXHIST")
    });

    let captured = app.world().resource::<Captured>().0.clone();
    teardown(&mut app);
    assert!(
        recalled,
        "ArrowUp must recall the previous command via history; the `-K` path \
         instead injected a literal char. Captured after Up: {:?}",
        String::from_utf8_lossy(&captured)
    );
}
