//! Gated end-to-end tests for the Phase 4 session-switch + chooser-query path
//! against a real tmux. Run with:
//! `cargo test -p ozmux_tmux --test real_tmux_switch -- --ignored`.

use std::time::Duration;
use tmux_control::{TmuxServer, WindowEntry};

fn unique_socket(tag: &str) -> String {
    format!("ozmux-phase4-{tag}-{}", std::process::id())
}

fn show_environment(socket: &str, session: &str, key: &str) -> std::process::Output {
    std::process::Command::new("tmux")
        .args(["-L", socket, "show-environment", "-t", session, key])
        .output()
        .expect("run tmux show-environment")
}

#[test]
#[ignore = "requires a real tmux binary"]
fn list_windows_all_spans_sessions() {
    let socket = unique_socket("lw");
    let server = TmuxServer::new().socket_name(&socket);
    let a = server.create_detached_session().expect("create a");
    let b = server.create_detached_session().expect("create b");
    std::thread::sleep(Duration::from_millis(200));

    let windows: Vec<WindowEntry> = server.list_windows_all().expect("list-windows -a");
    let names: Vec<&str> = windows.iter().map(|w| w.session_name.as_str()).collect();
    assert!(names.contains(&a.as_str()), "session a present: {names:?}");
    assert!(names.contains(&b.as_str()), "session b present: {names:?}");

    server
        .attach(&a)
        .map(|c| c.handle().send("kill-server").ok())
        .ok();
}

#[test]
#[ignore = "requires a real tmux binary and a controlling PTY"]
fn switch_client_emits_a_session_change() {
    let socket = unique_socket("sw");
    let server = TmuxServer::new().socket_name(&socket);
    let a = server.create_detached_session().expect("create a");
    let b = server.create_detached_session().expect("create b");

    let client = server.attach(&a).expect("attach a");
    std::thread::sleep(Duration::from_millis(400));
    while client.events().try_recv().is_ok() {}

    client
        .handle()
        .send(&ozmux_tmux::switch_client_command(&b))
        .expect("switch-client");
    std::thread::sleep(Duration::from_millis(400));

    let mut saw_session_change = false;
    while let Ok(ev) = client.events().try_recv() {
        if let tmux_control::TransportEvent::Protocol(tmux_control::ClientEvent::Notification(n)) =
            &ev
        {
            let s = format!("{n:?}");
            if s.contains("SessionChanged") || s.contains("ClientSessionChanged") {
                saw_session_change = true;
            }
        }
    }
    assert!(
        saw_session_change,
        "switch-client should emit a (client-)session-changed notification"
    );

    client.handle().send("kill-server").ok();
}

#[test]
#[ignore = "requires a real tmux binary"]
fn create_detached_session_with_env_sets_session_environment() {
    let socket = unique_socket("createenv");
    let server = TmuxServer::new()
        .socket_name(&socket)
        .env("OZMA_SOCK", "/tmp/ozma-create.sock");
    let name = server.create_detached_session().expect("create with env");
    std::thread::sleep(Duration::from_millis(200));

    let out = show_environment(&socket, &name, "OZMA_SOCK");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("OZMA_SOCK=/tmp/ozma-create.sock"),
        "new-session -e should set OZMA_SOCK in the session environment: {stdout:?} / {}",
        String::from_utf8_lossy(&out.stderr)
    );

    TmuxServer::new()
        .socket_name(&socket)
        .attach(&name)
        .map(|c| c.handle().send("kill-server").ok())
        .ok();
}

#[test]
#[ignore = "requires a real tmux binary and a controlling PTY"]
fn set_environment_in_session_command_reaches_only_the_target_session() {
    let socket = unique_socket("setenv");
    let server = TmuxServer::new().socket_name(&socket);
    let a = server.create_detached_session().expect("create a");
    let b = server.create_detached_session().expect("create b");

    // Attached to a, set OZMA_SOCK on b (the switch target) explicitly.
    let client = server.attach(&a).expect("attach a");
    std::thread::sleep(Duration::from_millis(300));
    client
        .handle()
        .send(&ozmux_tmux::set_environment_in_session_command(
            &b,
            "OZMA_SOCK",
            "/tmp/ozma-switch.sock",
        ))
        .expect("set-environment -t b");
    std::thread::sleep(Duration::from_millis(300));

    let on_b = show_environment(&socket, &b, "OZMA_SOCK");
    assert!(
        String::from_utf8_lossy(&on_b.stdout).contains("OZMA_SOCK=/tmp/ozma-switch.sock"),
        "target session b should carry OZMA_SOCK: {} / {}",
        String::from_utf8_lossy(&on_b.stdout),
        String::from_utf8_lossy(&on_b.stderr)
    );
    // The current session a must NOT receive it — proves the `-t` targeting that
    // a current-session set-environment would have gotten wrong.
    let on_a = show_environment(&socket, &a, "OZMA_SOCK");
    assert!(
        !String::from_utf8_lossy(&on_a.stdout).contains("OZMA_SOCK="),
        "current session a must not receive b's targeted env: {}",
        String::from_utf8_lossy(&on_a.stdout)
    );

    client.handle().send("kill-server").ok();
}
