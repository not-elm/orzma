//! Gated end-to-end test: DECSET mouse-mode bytes forwarded through `%output`.
//!
//! Closes the `[unverified]` premise in the arbiter design doc — namely that
//! tmux `-CC` `%output` carries the raw DECSET bytes a pane app writes, so
//! the detached `TerminalHandle` (`advance()` + `current_modes()`) is the
//! correct gate for mouse-report forwarding.
//!
//! Run with:
//!   `cargo test -p ozmux_tmux --test real_tmux_mouse -- --ignored --test-threads=1`
//!
//! Companion unit test proving the handle side independently:
//!   `cargo test -p ozma_tty_engine --test decset_mouse`

use bevy::prelude::*;
use ozmux_tmux::{ConnectionState, PaneOutput, TmuxConnection, TmuxPane, TmuxSessionPlugin};
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

/// Asserts that `%output` for a pane contains `needle` as a contiguous
/// byte subsequence.
fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// In a real tmux `-CC` session, a pane command that writes DECSET bytes
/// (`\x1b[?1000h\x1b[?1006h`) must be forwarded verbatim through `%output`.
///
/// This is a necessary condition for `TerminalHandle::advance()` +
/// `current_modes()` to serve as the correct gate for mouse-report forwarding.
#[test]
#[ignore = "requires a real tmux binary and a controlling PTY"]
fn decset_bytes_reach_pane_output() {
    let socket = format!("ozmux-mouse-{}", std::process::id());
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
    if !ready {
        // NOTE: tear the tmux -CC server down before unwinding, else the early
        // failure leaks the spawned server/socket across (ignored) test runs.
        teardown(&mut app);
        panic!("tmux should attach and project a pane within 5s");
    }

    let target = pane_q
        .iter(app.world())
        .next()
        .map(|p| format!("%{}", p.id.0))
        .expect("a projected pane");

    // Emit DECSET ?1000h (X10 mouse click) and ?1006h (SGR mouse) from the pane.
    // The shell quotes are handled by `send-keys -l` (literal, no interpretation).
    let cmd = format!("send-keys -t {target} -l -- 'printf \"\\033[?1000h\\033[?1006h\"'");
    handle_of(&app).send(&cmd).expect("send-keys printf");
    handle_of(&app)
        .send(&format!("send-keys -t {target} Enter"))
        .expect("send Enter");

    // Pump until both DECSET sequences appear in the accumulated %output bytes.
    let saw_decset = pump_until(&mut app, 10, |app| {
        let captured = &app.world().resource::<Captured>().0;
        contains_bytes(captured, b"\x1b[?1000h") && contains_bytes(captured, b"\x1b[?1006h")
    });

    let captured = app.world().resource::<Captured>().0.clone();
    teardown(&mut app);

    assert!(
        saw_decset,
        "tmux %output must carry the DECSET bytes \\x1b[?1000h and \\x1b[?1006h \
         verbatim so TerminalHandle::current_modes() can gate mouse forwarding; \
         captured {} bytes: {:?}",
        captured.len(),
        String::from_utf8_lossy(&captured)
    );
}
