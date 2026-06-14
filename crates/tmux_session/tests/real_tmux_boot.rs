//! Gated end-to-end test of the boot connect path against a real tmux.
//! Run with: `cargo test -p ozmux_tmux --test real_tmux_boot -- --ignored`.

use ozmux_tmux::{AttachTarget, attach_or_create, select_attach_target};
use std::time::Duration;
use tmux_control::TmuxServer;

#[test]
#[ignore = "requires a real tmux binary and a controlling PTY"]
fn select_then_connect_against_real_tmux() {
    let socket = format!("ozmux-phase1a-{}", std::process::id());
    let server = TmuxServer::new().socket_name(&socket);

    let sessions = server.list_sessions().expect("list (no server)");
    assert_eq!(select_attach_target(&sessions), AttachTarget::CreateNew);

    let created = attach_or_create(&server, &AttachTarget::CreateNew).expect("new_session");
    std::thread::sleep(Duration::from_millis(500));

    let sessions = server.list_sessions().expect("list (with server)");
    assert!(!sessions.is_empty(), "a session should now exist");
    assert!(
        matches!(select_attach_target(&sessions), AttachTarget::Attach(_)),
        "an existing (attached) session should be chosen for attach"
    );

    created.handle().send("kill-server").ok();
}
