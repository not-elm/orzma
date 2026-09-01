mod support;

use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::Rect;
use ratatui::widgets::StatefulWidget;
use ratatui_orzma::{Orzma, OrzmaBackend, OrzmaError, RpcError, Webview, WebviewWidget};
use serde_json::json;
use std::io::Write;
use std::sync::{Arc, Mutex};
use support::FakeServer;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn with_env(sock: &std::path::Path, f: impl FnOnce()) {
    // A panicking test poisons the lock; recover the guard so it doesn't cascade
    // and mask the test that actually failed.
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: ENV_LOCK serializes all callers; no other test thread touches these vars.
    unsafe {
        std::env::set_var("ORZMA_SOCK", sock);
        std::env::set_var("ORZMA_TOKEN", "test-token");
    }
    f();
    unsafe {
        std::env::remove_var("ORZMA_SOCK");
        std::env::remove_var("ORZMA_TOKEN");
    }
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
fn backend_draw_emits_mount_apc_and_focus_op() {
    let server = FakeServer::start("view-1");
    with_env(&server.sock_path.clone(), || {
        let orzma = Orzma::connect().unwrap();
        let handle = orzma.register(Webview::inline("x")).unwrap();

        let term_bytes = SharedBuf(Arc::new(Mutex::new(Vec::new())));
        let mut backend = OrzmaBackend::new(CrosstermBackend::new(term_bytes.clone()), &orzma);

        // A WebviewWidget records its placement + focus into the frame the SDK
        // shares with the backend — the same path render_stateful_widget drives.
        {
            let mut scratch = Buffer::empty(Rect::new(0, 0, 80, 40));
            let mut frame = orzma.frame();
            WebviewWidget::new(handle.instance_id())
                .focused(true)
                .render(Rect::new(2, 3, 48, 12), &mut scratch, &mut *frame);
        }

        // Terminal::flush calls Backend::draw once per frame; drive it directly.
        let no_cells: Vec<(u16, u16, &Cell)> = Vec::new();
        Backend::draw(&mut backend, no_cells.into_iter()).unwrap();

        let out = String::from_utf8(term_bytes.0.lock().unwrap().clone()).unwrap();
        let instance = &server.instance;
        assert!(
            out.contains(&format!("Omount;n={instance},r=12,c=48")),
            "terminal output missing mount APC verb: {out:?}"
        );

        let msg = server.next_message();
        assert_eq!(msg["op"], "focus");
        assert_eq!(msg["instance"], instance.as_str());
    });
}

#[test]
fn new_instance_mints_a_second_placement_that_mounts_on_its_own() {
    let server = FakeServer::start("view-multi");
    with_env(&server.sock_path.clone(), || {
        let orzma = Orzma::connect().unwrap();
        let handle = orzma.register(Webview::inline("x")).unwrap();
        let extra = orzma.new_instance(&handle).unwrap();
        assert_ne!(
            extra.id(),
            handle.instance_id(),
            "an extra placement must not reuse the default instance"
        );

        let term_bytes = SharedBuf(Arc::new(Mutex::new(Vec::new())));
        let mut backend = OrzmaBackend::new(CrosstermBackend::new(term_bytes.clone()), &orzma);
        {
            let mut scratch = Buffer::empty(Rect::new(0, 0, 80, 40));
            let mut frame = orzma.frame();
            WebviewWidget::new(handle.instance_id()).render(
                Rect::new(0, 0, 10, 5),
                &mut scratch,
                &mut frame,
            );
            WebviewWidget::new(extra.id()).render(Rect::new(0, 6, 10, 5), &mut scratch, &mut frame);
        }
        Backend::draw(&mut backend, std::iter::empty::<(u16, u16, &Cell)>()).unwrap();

        let out = String::from_utf8(term_bytes.0.lock().unwrap().clone()).unwrap();
        let default = handle.instance_id();
        let second = extra.id();
        assert!(
            out.contains(&format!("Omount;n={default},")),
            "the default placement did not mount: {out:?}"
        );
        assert!(
            out.contains(&format!("Omount;n={second},")),
            "the minted placement did not mount: {out:?}"
        );
    });
}

#[test]
fn reconnect_remints_every_instance_of_a_registration() {
    use std::time::Duration;
    let pair = support::ReconnectPair::start("view-mi1", "view-mi2");
    with_env(&pair.first.sock_path.clone(), || {
        let orzma = Orzma::connect().unwrap();
        let handle = orzma.register(Webview::inline("x")).unwrap();
        let extra = orzma.new_instance(&handle).unwrap();
        let before = extra.id();

        let term_bytes = SharedBuf(Arc::new(Mutex::new(Vec::new())));
        let mut backend = OrzmaBackend::new(CrosstermBackend::new(term_bytes.clone()), &orzma);
        Backend::draw(&mut backend, std::iter::empty::<(u16, u16, &Cell)>()).unwrap();

        drop(pair.first);
        std::thread::sleep(Duration::from_millis(200));
        // NOTE: ENV_LOCK is held by with_env, serializing env var access.
        unsafe { std::env::set_var("ORZMA_SOCK", &pair.second.sock_path) };
        Backend::draw(&mut backend, std::iter::empty::<(u16, u16, &Cell)>()).unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while extra.id() == before {
            assert!(
                std::time::Instant::now() < deadline,
                "the extra placement was never re-minted"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(
            handle.instance_id(),
            pair.second.instance,
            "the default placement must follow the re-registration"
        );
        assert_eq!(
            extra.id(),
            support::instance_for("view-mi2/1"),
            "the extra placement must be re-minted on the new connection"
        );
    });
}

#[test]
fn call_is_dispatched_and_replied() {
    let server = FakeServer::start("view-1");
    with_env(&server.sock_path.clone(), || {
        let orzma = Orzma::connect().unwrap();
        let _handle = orzma
            .register(Webview::inline("<h1>x</h1>").on("ping", |n: String| Ok(format!("pong:{n}"))))
            .unwrap();

        server.send(json!({
            "op": "call", "handle": "view-1", "instance": server.instance,
            "reqId": "7", "method": "ping", "params": "hi"
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
        let orzma = Orzma::connect().unwrap();
        let _h = orzma.register(Webview::inline("x")).unwrap();
        server.send(json!({
            "op": "call", "handle": "view-2", "instance": server.instance,
            "reqId": "1", "method": "nope", "params": null
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
        let orzma = Orzma::connect().unwrap();
        let handle = orzma.register(Webview::inline("x")).unwrap();
        handle.emit("tick", &42u32).unwrap();
        let msg = server.next_message();
        assert_eq!(msg["op"], "emit");
        assert_eq!(msg["handle"], "view-3");
        assert_eq!(msg["event"], "tick");
        assert_eq!(msg["payload"], 42);
    });
}

#[test]
fn inbound_event_is_buffered_and_read() {
    use std::time::{Duration, Instant};
    #[derive(serde::Deserialize, PartialEq, Debug)]
    struct Hello {
        message: String,
    }

    let server = FakeServer::start("view-ev");
    with_env(&server.sock_path.clone(), || {
        let orzma = Orzma::connect().unwrap();
        let handle = orzma
            .register(Webview::inline("x").add_event::<Hello>("hello"))
            .unwrap();

        server.send(json!({
            "op": "event", "handle": "view-ev", "event": "hello", "payload": { "message": "hi" }
        }));

        let deadline = Instant::now() + Duration::from_secs(5);
        let events = loop {
            let evs = handle.read_events::<Hello>();
            if !evs.is_empty() {
                break evs;
            }
            assert!(Instant::now() < deadline, "inbound event never arrived");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(
            events,
            vec![Hello {
                message: "hi".into()
            }]
        );
    });
}

#[test]
fn register_returns_disconnected_when_socket_closes() {
    // Regression: a register whose reply never arrives because the socket closes
    // must return Disconnected, not block forever on the pending reply.
    let server = FakeServer::start_dropping();
    with_env(&server.sock_path.clone(), || {
        let orzma = Orzma::connect().unwrap();
        assert!(matches!(
            orzma.register(Webview::inline("x")),
            Err(OrzmaError::Disconnected)
        ));
    });
}

#[test]
fn panicking_handler_does_not_kill_reader() {
    // Regression: a panicking handler must report a rejected call and leave the
    // reader thread alive to serve subsequent calls.
    let server = FakeServer::start("view-5");
    with_env(&server.sock_path.clone(), || {
        let orzma = Orzma::connect().unwrap();
        let _h = orzma
            .register(
                Webview::inline("x")
                    .on("boom", |_: ()| -> Result<(), RpcError> { panic!("boom") })
                    .on("ping", |_: ()| Ok::<_, RpcError>("pong")),
            )
            .unwrap();

        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        server.send(json!({
            "op": "call", "handle": "view-5", "instance": server.instance,
            "reqId": "1", "method": "boom", "params": null
        }));
        let boom = server.next_message();
        std::panic::set_hook(prev);
        assert_eq!(boom["reqId"], "1");
        assert_eq!(boom["ok"], false);

        server.send(json!({
            "op": "call", "handle": "view-5", "instance": server.instance,
            "reqId": "2", "method": "ping", "params": null
        }));
        let ping = server.next_message();
        assert_eq!(ping["reqId"], "2");
        assert_eq!(ping["ok"], true);
        assert_eq!(ping["value"], "pong");
    });
}

#[test]
fn reconnect_updates_handle_id_and_reregisters() {
    use std::time::Duration;
    let pair = support::ReconnectPair::start("view-rc1", "view-rc2");
    with_env(&pair.first.sock_path.clone(), || {
        let orzma = Orzma::connect().unwrap();
        let handle = orzma.register(Webview::inline("<h1>rc</h1>")).unwrap();
        assert_eq!(
            handle.handle_id().to_string(),
            "view-rc1",
            "initial registration must get first handle"
        );

        let term_bytes = SharedBuf(Arc::new(Mutex::new(Vec::new())));
        let mut backend = OrzmaBackend::new(CrosstermBackend::new(term_bytes.clone()), &orzma);
        Backend::draw(&mut backend, std::iter::empty::<(u16, u16, &Cell)>()).unwrap();

        drop(pair.first);

        std::thread::sleep(Duration::from_millis(200));

        // NOTE: ENV_LOCK is held by with_env, serializing all env var accesses.
        unsafe { std::env::set_var("ORZMA_SOCK", &pair.second.sock_path) };

        Backend::draw(&mut backend, std::iter::empty::<(u16, u16, &Cell)>()).unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while handle.handle_id().to_string() == "view-rc1" {
            assert!(
                std::time::Instant::now() < deadline,
                "reconnect did not complete within 5 seconds"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(
            handle.handle_id().to_string(),
            "view-rc2",
            "handle ID must update to second server's handle after reconnect"
        );
    });
}

#[test]
fn connect_reports_stale_socket_as_unavailable_not_io() {
    // A stale $ORZMA_SOCK inherited from an exited orzma points at a removed
    // control dir. connect must surface SocketUnavailable so the caller can tell
    // the user to restart orzma — not a bare Io error nor the misleading
    // "not in a pane" hint (the user IS in a pane).
    let dead = std::env::temp_dir().join(format!("orzma-dead-{}/control.sock", std::process::id()));
    let _ = std::fs::remove_dir_all(dead.parent().unwrap());
    with_env(&dead, || {
        let connected = Orzma::connect();
        assert!(
            matches!(connected, Err(OrzmaError::SocketUnavailable { .. })),
            "a dead $ORZMA_SOCK must report SocketUnavailable, got: {:?}",
            connected.err()
        );
    });
}

#[test]
fn reconnect_preserves_inbound_events() {
    use std::time::Duration;
    #[derive(serde::Deserialize, PartialEq, Debug)]
    struct Hello {
        message: String,
    }

    let pair = support::ReconnectPair::start("view-ev1", "view-ev2");
    with_env(&pair.first.sock_path.clone(), || {
        let orzma = Orzma::connect().unwrap();
        let handle = orzma
            .register(Webview::inline("x").add_event::<Hello>("hello"))
            .unwrap();

        let term_bytes = SharedBuf(Arc::new(Mutex::new(Vec::new())));
        let mut backend = OrzmaBackend::new(CrosstermBackend::new(term_bytes.clone()), &orzma);
        Backend::draw(&mut backend, std::iter::empty::<(u16, u16, &Cell)>()).unwrap();

        drop(pair.first);
        std::thread::sleep(Duration::from_millis(200));
        // NOTE: ENV_LOCK is held by with_env, serializing env var access.
        unsafe { std::env::set_var("ORZMA_SOCK", &pair.second.sock_path) };
        Backend::draw(&mut backend, std::iter::empty::<(u16, u16, &Cell)>()).unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while handle.handle_id().to_string() == "view-ev1" {
            assert!(
                std::time::Instant::now() < deadline,
                "reconnect did not complete"
            );
            std::thread::sleep(Duration::from_millis(50));
        }

        // The page emits to the NEW handle after reconnect; read_events must see it.
        pair.second.send(json!({
            "op": "event", "handle": "view-ev2", "event": "hello", "payload": { "message": "post" }
        }));

        // A fresh deadline: the reconnect loop above may have consumed most of the
        // first budget on a slow machine, which must not starve this wait.
        let event_deadline = std::time::Instant::now() + Duration::from_secs(5);
        let got = loop {
            let evs = handle.read_events::<Hello>();
            if !evs.is_empty() {
                break evs;
            }
            assert!(
                std::time::Instant::now() < event_deadline,
                "event never arrived after reconnect"
            );
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(
            got,
            vec![Hello {
                message: "post".into()
            }]
        );
    });
}
