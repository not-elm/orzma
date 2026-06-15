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
    // SAFETY: ENV_LOCK serializes all callers; no other test thread touches these vars.
    unsafe {
        std::env::set_var("OZMUX_SOCK", sock);
        std::env::set_var("OZMUX_TOKEN", "test-token");
    }
    f();
    unsafe {
        std::env::remove_var("OZMUX_SOCK");
        std::env::remove_var("OZMUX_TOKEN");
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
            WebviewWidget::new(handle.id()).focused(true).render(
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
            .register(
                Webview::inline("<h1>x</h1>").on("ping", |(n,): (String,)| Ok(format!("pong:{n}"))),
            )
            .unwrap();

        server.send(json!({
            "op": "call", "handle": "view-1", "reqId": "7", "method": "ping", "args": ["hi"]
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
            "op": "call", "handle": "view-2", "reqId": "1", "method": "nope", "args": []
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
                    .on("boom", |(): ()| -> Result<(), RpcError> { panic!("boom") })
                    .on("ping", |(): ()| Ok::<_, RpcError>("pong")),
            )
            .unwrap();

        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        server.send(
            json!({ "op": "call", "handle": "view-5", "reqId": "1", "method": "boom", "args": [] }),
        );
        let boom = server.next_message();
        std::panic::set_hook(prev);
        assert_eq!(boom["reqId"], "1");
        assert_eq!(boom["ok"], false);

        server.send(
            json!({ "op": "call", "handle": "view-5", "reqId": "2", "method": "ping", "args": [] }),
        );
        let ping = server.next_message();
        assert_eq!(ping["reqId"], "2");
        assert_eq!(ping["ok"], true);
        assert_eq!(ping["value"], "pong");
    });
}
