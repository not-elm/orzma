//! Gated end-to-end tests against a real tmux `-CC`: drive `TmuxSessionPlugin`
//! until it has read the key bindings, then dispatch keypresses via the public
//! `plan_forward` and assert bound commands run while unbound keys forward.
//! The tests force prefix=C-b and bind their own keys, so they're independent
//! of the developer's ~/.tmux.conf (the only remaining assumption is that the
//! config doesn't bind `-n Up`, which would shadow the unbound-key test).
//! Run with: `cargo test -p ozmux_tmux --test real_tmux_keybindings -- --ignored`.

use bevy::prelude::*;
use ozmux_tmux::{
    ConnectionState, Forwarded, KeyBindings, TmuxConnection, TmuxPane, TmuxSessionPlugin,
    plan_forward,
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

fn run_cmd(app: &App, cmd: &str) {
    app.world()
        .get_non_send_resource::<TmuxConnection>()
        .unwrap()
        .client()
        .unwrap()
        .handle()
        .send(cmd)
        .expect("send command");
}

fn pane_count(app: &mut App) -> usize {
    let mut q = app.world_mut().query::<&TmuxPane>();
    q.iter(app.world()).count()
}

/// True once the prefix-key query reply (the last of the three on-attach
/// keybinding reads) has landed — which, because replies are FIFO, implies the
/// root and prefix tables are already installed too.
fn bindings_ready(app: &App) -> bool {
    let kb = app.world().resource::<KeyBindings>();
    let mut pending = false;
    plan_forward(&mut pending, kb, vec!["C-b".to_string()]);
    pending
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

/// Attaches an app to a fresh `-CC` session that already has `extra_binding`
/// installed, pumping until a pane is projected and all key bindings are read.
fn attach_with_binding(tag: &str, extra_binding: &str) -> App {
    let socket = format!("ozmux-kb-{}-{}", std::process::id(), tag);
    let server = TmuxServer::new().socket_name(&socket);
    let client = server.new_session().expect("spawn tmux -CC new-session");
    client
        .handle()
        .send(extra_binding)
        .expect("install binding");
    // Force a known prefix so the tests are independent of the developer's
    // ~/.tmux.conf (a fresh tmux server still sources it).
    client
        .handle()
        .send("set -g prefix C-b")
        .expect("force prefix");
    client
        .handle()
        .send("set -g prefix2 None")
        .expect("force prefix2");

    let mut app = App::new();
    app.add_plugins(TmuxSessionPlugin);
    app.world_mut()
        .get_non_send_resource_mut::<TmuxConnection>()
        .expect("TmuxConnection inserted by the plugin")
        .set(client);

    let ready = pump_until(&mut app, 5, |app| {
        let attached = *app.world().resource::<ConnectionState>() == ConnectionState::Attached;
        let mut q = app.world_mut().query::<&TmuxPane>();
        let has_pane = q.iter(app.world()).next().is_some();
        attached && has_pane && bindings_ready(app)
    });
    assert!(
        ready,
        "tmux should attach, project a pane, and read bindings within 5s"
    );
    app
}

#[test]
#[ignore = "requires a real tmux binary and a controlling PTY"]
fn root_binding_runs_command_and_splits() {
    let mut app = attach_with_binding("root", "bind-key -n M-i split-window -h");
    assert_eq!(pane_count(&mut app), 1);

    let cmd = {
        let kb = app.world().resource::<KeyBindings>();
        match plan_forward(&mut false, kb, vec!["M-i".to_string()])
            .into_iter()
            .next()
        {
            Some(Forwarded::Run(cmd)) => cmd,
            other => panic!("expected Run for bound M-i, got {other:?}"),
        }
    };
    assert_eq!(cmd, "split-window -h");
    run_cmd(&app, &cmd);

    let split = pump_until(&mut app, 3, |app| pane_count(app) == 2);
    teardown(&mut app);
    assert!(
        split,
        "running the bound split-window command must create a second pane"
    );
}

#[test]
#[ignore = "requires a real tmux binary and a controlling PTY"]
fn prefix_binding_runs_after_prefix_key() {
    // Default tmux: prefix is C-b and `bind c new-window` is a default binding.
    let mut app = attach_with_binding("prefix", "bind-key -T prefix c new-window");

    let result = {
        let kb = app.world().resource::<KeyBindings>();
        let mut pending = false;
        let first = plan_forward(&mut pending, kb, vec!["C-b".to_string()]);
        assert!(first.is_empty(), "the prefix key is swallowed");
        assert!(pending, "prefix is pending after C-b");
        plan_forward(&mut pending, kb, vec!["c".to_string()])
            .into_iter()
            .next()
    };
    teardown(&mut app);
    match result {
        Some(Forwarded::Run(cmd)) => assert!(cmd.contains("new-window"), "got: {cmd}"),
        other => panic!("prefix + c should run the default new-window binding, got {other:?}"),
    }
}

#[test]
#[ignore = "requires a real tmux binary and a controlling PTY"]
fn unbound_key_is_forwarded() {
    let mut app = attach_with_binding("unbound", "bind-key -n M-i split-window -h");
    let result = {
        let kb = app.world().resource::<KeyBindings>();
        plan_forward(&mut false, kb, vec!["Up".to_string()])
            .into_iter()
            .next()
    };
    teardown(&mut app);
    match result {
        Some(Forwarded::Keys(keys)) => assert_eq!(keys, vec!["Up".to_string()]),
        other => panic!("an unbound key must be forwarded to the pane, got {other:?}"),
    }
}
