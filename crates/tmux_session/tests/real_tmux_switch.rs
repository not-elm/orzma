//! Gated end-to-end tests for the Phase 4 session-switch + chooser-query path
//! against a real tmux. Run with:
//! `cargo test -p ozmux_tmux --test real_tmux_switch -- --ignored`.

use std::time::Duration;
use tmux_control::{TmuxServer, WindowEntry};

fn unique_socket(tag: &str) -> String {
    format!("ozmux-phase4-{tag}-{}", std::process::id())
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
