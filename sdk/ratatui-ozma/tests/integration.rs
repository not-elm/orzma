mod support;

use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::Rect;
use ratatui::widgets::StatefulWidget;
use ratatui_ozma::{Ozma, OzmaBackend, OzmaError, RpcError, Webview, WebviewWidget};
use serde_json::json;
use std::io::Write;
use std::sync::{Arc, Mutex};
use support::FakeServer;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn with_env(sock: &std::path::Path, f: impl FnOnce()) {
    // A panicking test poisons the lock; recover the guard so it doesn't cascade
    // and mask the test that actually failed.
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // NOTE: clear $TMUX so resolve prefers ONLY the injected $OZMA_SOCK. connect()
    // now consults the tmux session env first; if the test process is itself run
    // inside tmux (CI/dev in a pane), an ambient $TMUX would otherwise hijack
    // resolution to the real server instead of this test's FakeServer.
    let prev_tmux = std::env::var_os("TMUX");
    // SAFETY: ENV_LOCK serializes all callers; no other test thread touches these vars.
    unsafe {
        std::env::set_var("OZMA_SOCK", sock);
        std::env::set_var("OZMA_TOKEN", "test-token");
        std::env::remove_var("TMUX");
    }
    f();
    unsafe {
        std::env::remove_var("OZMA_SOCK");
        std::env::remove_var("OZMA_TOKEN");
    }
    set_or_remove("TMUX", prev_tmux);
}

#[derive(Clone)]
struct SharedBuf(Arc<Mutex<Vec<u8>>>);

impl Write for SharedBuf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn backend_draw_emits_mount_osc_and_focus_op() {
    let server = FakeServer::start("view-1");
    with_env(&server.sock_path.clone(), || {
        let ozma = Ozma::connect().unwrap();
        let handle = ozma.register(Webview::inline("x")).unwrap();

        let term_bytes = SharedBuf(Arc::new(Mutex::new(Vec::new())));
        let mut backend = OzmaBackend::new(CrosstermBackend::new(term_bytes.clone()), &ozma);

        // A WebviewWidget records its placement + focus into the frame the SDK
        // shares with the backend — the same path render_stateful_widget drives.
        {
            let mut scratch = Buffer::empty(Rect::new(0, 0, 80, 40));
            let mut frame = ozma.frame();
            WebviewWidget::new(&handle.id()).focused(true).render(
                Rect::new(2, 3, 48, 12),
                &mut scratch,
                &mut *frame,
            );
        }

        // Terminal::flush calls Backend::draw once per frame; drive it directly.
        let no_cells: Vec<(u16, u16, &Cell)> = Vec::new();
        Backend::draw(&mut backend, no_cells.into_iter()).unwrap();

        let out = String::from_utf8(term_bytes.0.lock().unwrap().clone()).unwrap();
        assert!(
            out.contains("mount-inline;view-1;12;48"),
            "terminal output missing mount OSC: {out:?}"
        );

        let msg = server.next_message();
        assert_eq!(msg["op"], "focus");
        assert_eq!(msg["handle"], "view-1");
    });
}

#[test]
fn call_is_dispatched_and_replied() {
    let server = FakeServer::start("view-1");
    with_env(&server.sock_path.clone(), || {
        let ozma = Ozma::connect().unwrap();
        let _handle = ozma
            .register(Webview::inline("<h1>x</h1>").on("ping", |n: String| Ok(format!("pong:{n}"))))
            .unwrap();

        server.send(json!({
            "op": "call", "handle": "view-1", "reqId": "7", "method": "ping", "params": "hi"
        }));

        let reply = server.next_message();
        assert_eq!(reply["op"], "reply");
        assert_eq!(reply["reqId"], "7");
        assert_eq!(reply["ok"], true);
        assert_eq!(reply["value"], "pong:hi");
    });
}

#[test]
fn unknown_method_replies_error() {
    let server = FakeServer::start("view-2");
    with_env(&server.sock_path.clone(), || {
        let ozma = Ozma::connect().unwrap();
        let _h = ozma.register(Webview::inline("x")).unwrap();
        server.send(json!({
            "op": "call", "handle": "view-2", "reqId": "1", "method": "nope", "params": null
        }));
        let reply = server.next_message();
        assert_eq!(reply["ok"], false);
        assert_eq!(reply["error"], "unknown_method");
    });
}

#[test]
fn emit_reaches_the_server() {
    let server = FakeServer::start("view-3");
    with_env(&server.sock_path.clone(), || {
        let ozma = Ozma::connect().unwrap();
        let handle = ozma.register(Webview::inline("x")).unwrap();
        handle.emit("tick", &42u32).unwrap();
        let msg = server.next_message();
        assert_eq!(msg["op"], "emit");
        assert_eq!(msg["handle"], "view-3");
        assert_eq!(msg["event"], "tick");
        assert_eq!(msg["payload"], 42);
    });
}

#[test]
fn register_returns_disconnected_when_socket_closes() {
    // Regression: a register whose reply never arrives because the socket closes
    // must return Disconnected, not block forever on the pending reply.
    let server = FakeServer::start_dropping();
    with_env(&server.sock_path.clone(), || {
        let ozma = Ozma::connect().unwrap();
        assert!(matches!(
            ozma.register(Webview::inline("x")),
            Err(OzmaError::Disconnected)
        ));
    });
}

#[test]
fn panicking_handler_does_not_kill_reader() {
    // Regression: a panicking handler must report a rejected call and leave the
    // reader thread alive to serve subsequent calls.
    let server = FakeServer::start("view-5");
    with_env(&server.sock_path.clone(), || {
        let ozma = Ozma::connect().unwrap();
        let _h = ozma
            .register(
                Webview::inline("x")
                    .on("boom", |_: ()| -> Result<(), RpcError> { panic!("boom") })
                    .on("ping", |_: ()| Ok::<_, RpcError>("pong")),
            )
            .unwrap();

        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        server.send(
            json!({ "op": "call", "handle": "view-5", "reqId": "1", "method": "boom", "params": null }),
        );
        let boom = server.next_message();
        std::panic::set_hook(prev);
        assert_eq!(boom["reqId"], "1");
        assert_eq!(boom["ok"], false);

        server.send(
            json!({ "op": "call", "handle": "view-5", "reqId": "2", "method": "ping", "params": null }),
        );
        let ping = server.next_message();
        assert_eq!(ping["reqId"], "2");
        assert_eq!(ping["ok"], true);
        assert_eq!(ping["value"], "pong");
    });
}

// A private tmux server for the gated fallback test: kills the server and
// removes its socket on drop so a panicking assertion cannot leak it.
struct TmuxServerGuard {
    socket: std::path::PathBuf,
}

impl TmuxServerGuard {
    fn run(&self, args: &[&str]) -> std::process::Output {
        std::process::Command::new("tmux")
            .arg("-S")
            .arg(&self.socket)
            .args(args)
            .output()
            .expect("run tmux")
    }
}

impl Drop for TmuxServerGuard {
    fn drop(&mut self) {
        let _ = self.run(&["kill-server"]);
        let _ = std::fs::remove_file(&self.socket);
    }
}

fn set_or_remove(key: &str, val: Option<std::ffi::OsString>) {
    // SAFETY: callers hold ENV_LOCK; no other test thread touches these vars.
    unsafe {
        match val {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
    }
}

#[test]
#[ignore = "requires a real tmux binary"]
fn connect_resolves_ozma_sock_from_tmux_when_env_unset() {
    // The control socket the program connects to once it discovers the path.
    let server = FakeServer::start("view-fallback");
    let server_sock = server.sock_path.to_string_lossy().into_owned();

    // A detached tmux session forks its pane shell now — before set-environment —
    // so the shell never inherits OZMA_SOCK (the pre-existing-pane scenario).
    let tmux_socket =
        std::env::temp_dir().join(format!("ozma-resolve-{}.tmuxsock", std::process::id()));
    let _ = std::fs::remove_file(&tmux_socket);
    let tmux = TmuxServerGuard {
        socket: tmux_socket.clone(),
    };
    let created = tmux.run(&["new-session", "-d", "-s", "ozmares"]);
    assert!(
        created.status.success(),
        "tmux new-session failed: {}",
        String::from_utf8_lossy(&created.stderr)
    );

    // ozma's post-attach injection: set OZMA_SOCK in the session environment.
    let set = tmux.run(&[
        "set-environment",
        "-t",
        "ozmares",
        "OZMA_SOCK",
        &server_sock,
    ]);
    assert!(
        set.status.success(),
        "set-environment failed: {}",
        String::from_utf8_lossy(&set.stderr)
    );

    // Reconstruct the $TMUX a pane carries: <socket>,<server-pid>,<session-id>.
    let session_field = |format: &str| {
        let out = tmux.run(&["display-message", "-p", "-t", "ozmares", format]);
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    };
    let sid = session_field("#{session_id}");
    let sid = sid.trim_start_matches('$');
    let pid = session_field("#{pid}");
    let tmux_env = format!("{},{},{}", tmux_socket.display(), pid, sid);

    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev_sock = std::env::var_os("OZMA_SOCK");
    let prev_token = std::env::var_os("OZMA_TOKEN");
    let prev_tmux = std::env::var_os("TMUX");
    // SAFETY: ENV_LOCK serializes all callers; no other test thread touches these vars.
    unsafe {
        std::env::remove_var("OZMA_SOCK");
        std::env::set_var("OZMA_TOKEN", "fallback-token");
        std::env::set_var("TMUX", &tmux_env);
    }

    let connected = Ozma::connect();

    set_or_remove("OZMA_SOCK", prev_sock);
    set_or_remove("OZMA_TOKEN", prev_token);
    set_or_remove("TMUX", prev_tmux);

    assert!(
        connected.is_ok(),
        "connect should resolve OZMA_SOCK via tmux show-environment: {:?}",
        connected.err()
    );
}

#[test]
fn connect_reports_stale_socket_as_unavailable_not_io() {
    // A stale $OZMA_SOCK (left in the tmux env by an exited ozmux) points at a
    // removed control dir. connect must surface SocketUnavailable so the caller
    // can tell the user to re-attach ozmux — not a bare Io error nor the
    // misleading "not in a pane" hint (the user IS in a pane).
    let dead = std::env::temp_dir().join(format!("ozma-dead-{}/control.sock", std::process::id()));
    let _ = std::fs::remove_dir_all(dead.parent().unwrap());
    with_env(&dead, || {
        let connected = Ozma::connect();
        assert!(
            matches!(connected, Err(OzmaError::SocketUnavailable { .. })),
            "a dead $OZMA_SOCK must report SocketUnavailable, got: {:?}",
            connected.err()
        );
    });
}

#[test]
#[ignore = "requires a real tmux binary"]
fn connect_reports_stale_socket_resolved_from_tmux() {
    // The exact user scenario: a pre-existing pane resolves OZMA_SOCK from the
    // tmux session env, but the value is stale (the ozmux that set it has exited
    // and its control dir is gone). The SDK must report SocketUnavailable.
    let dead_sock = std::env::temp_dir()
        .join(format!("ozma-stale-{}/control.sock", std::process::id()))
        .to_string_lossy()
        .into_owned();
    let _ = std::fs::remove_dir_all(std::path::Path::new(&dead_sock).parent().unwrap());

    let (_tmux, tmux_env) = tmux_with_ozma_sock("stale", &dead_sock);

    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev_sock = std::env::var_os("OZMA_SOCK");
    let prev_token = std::env::var_os("OZMA_TOKEN");
    let prev_tmux = std::env::var_os("TMUX");
    // SAFETY: ENV_LOCK serializes all callers; no other test thread touches these vars.
    unsafe {
        std::env::remove_var("OZMA_SOCK");
        std::env::set_var("OZMA_TOKEN", "stale-token");
        std::env::set_var("TMUX", &tmux_env);
    }

    let connected = Ozma::connect();

    set_or_remove("OZMA_SOCK", prev_sock);
    set_or_remove("OZMA_TOKEN", prev_token);
    set_or_remove("TMUX", prev_tmux);

    assert!(
        matches!(connected, Err(OzmaError::SocketUnavailable { .. })),
        "a stale tmux-resolved sock must report SocketUnavailable, got: {:?}",
        connected.err()
    );
}

// Starts a private tmux server with OZMA_SOCK=`value` on a detached session and
// returns the guard plus the `$TMUX` string a pane in that session would carry.
fn tmux_with_ozma_sock(label: &str, value: &str) -> (TmuxServerGuard, String) {
    let tmux_socket =
        std::env::temp_dir().join(format!("ozma-{label}-{}.tmuxsock", std::process::id()));
    let _ = std::fs::remove_file(&tmux_socket);
    let tmux = TmuxServerGuard {
        socket: tmux_socket.clone(),
    };
    let created = tmux.run(&["new-session", "-d", "-s", "ozmasess"]);
    assert!(
        created.status.success(),
        "tmux new-session failed: {}",
        String::from_utf8_lossy(&created.stderr)
    );
    let set = tmux.run(&["set-environment", "-t", "ozmasess", "OZMA_SOCK", value]);
    assert!(
        set.status.success(),
        "set-environment failed: {}",
        String::from_utf8_lossy(&set.stderr)
    );
    let field = |f: &str| {
        let out = tmux.run(&["display-message", "-p", "-t", "ozmasess", f]);
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    };
    let sid = field("#{session_id}");
    let sid = sid.trim_start_matches('$');
    let pid = field("#{pid}");
    (tmux, format!("{},{},{}", tmux_socket.display(), pid, sid))
}

#[test]
#[ignore = "requires a real tmux binary"]
fn connect_prefers_live_tmux_value_over_stale_env() {
    // The reported bug: the pane inherited a STALE $OZMA_SOCK (an exited ozmux),
    // while the attached ozmux refreshed the tmux session env to its LIVE socket.
    // connect() must prefer/validate and reach the live socket, not the dead env.
    let live = FakeServer::start("view-pref");
    let live_sock = live.sock_path.to_string_lossy().into_owned();
    let dead = std::env::temp_dir().join(format!("ozma-prefdead-{}/x.sock", std::process::id()));
    let _ = std::fs::remove_dir_all(dead.parent().unwrap());
    let (_tmux, tmux_env) = tmux_with_ozma_sock("pref", &live_sock);

    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev_sock = std::env::var_os("OZMA_SOCK");
    let prev_token = std::env::var_os("OZMA_TOKEN");
    let prev_tmux = std::env::var_os("TMUX");
    // SAFETY: ENV_LOCK serializes all callers; no other test thread touches these vars.
    unsafe {
        std::env::set_var("OZMA_SOCK", &dead);
        std::env::set_var("OZMA_TOKEN", "pref-token");
        std::env::set_var("TMUX", &tmux_env);
    }

    let connected = Ozma::connect();

    set_or_remove("OZMA_SOCK", prev_sock);
    set_or_remove("OZMA_TOKEN", prev_token);
    set_or_remove("TMUX", prev_tmux);

    assert!(
        connected.is_ok(),
        "must reach the live tmux socket despite a stale $OZMA_SOCK, got: {:?}",
        connected.err()
    );
}

#[test]
#[ignore = "requires a real tmux binary"]
fn connect_falls_back_to_live_env_when_tmux_value_dead() {
    // Inverse: the tmux session value is dead but the inherited $OZMA_SOCK is live.
    // try-each must skip the dead tmux candidate and reach the live env one.
    let live = FakeServer::start("view-envlive");
    let live_sock = live.sock_path.to_string_lossy().into_owned();
    let dead = std::env::temp_dir().join(format!("ozma-tmuxdead-{}/x.sock", std::process::id()));
    let _ = std::fs::remove_dir_all(dead.parent().unwrap());
    let (_tmux, tmux_env) = tmux_with_ozma_sock("tmuxdead", &dead.to_string_lossy());

    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev_sock = std::env::var_os("OZMA_SOCK");
    let prev_token = std::env::var_os("OZMA_TOKEN");
    let prev_tmux = std::env::var_os("TMUX");
    // SAFETY: ENV_LOCK serializes all callers; no other test thread touches these vars.
    unsafe {
        std::env::set_var("OZMA_SOCK", &live_sock);
        std::env::set_var("OZMA_TOKEN", "envlive-token");
        std::env::set_var("TMUX", &tmux_env);
    }

    let connected = Ozma::connect();

    set_or_remove("OZMA_SOCK", prev_sock);
    set_or_remove("OZMA_TOKEN", prev_token);
    set_or_remove("TMUX", prev_tmux);

    assert!(
        connected.is_ok(),
        "must fall back to the live $OZMA_SOCK when the tmux value is dead, got: {:?}",
        connected.err()
    );
}
