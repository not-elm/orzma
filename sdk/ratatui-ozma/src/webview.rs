//! Webview builder and registered handle.

use crate::error::OzmaResult;
use crate::handler::{BoxedHandler, make_handler};
use crate::keymap::NavKeymap;
use crate::protocol::{ClientMsg, RegisterKind};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// The shared write half of the control socket.
pub(crate) type SharedWriter = Arc<Mutex<UnixStream>>;

/// A webview definition: content plus RPC handlers, before registration.
pub struct Webview {
    pub(crate) kind: RegisterKind,
    pub(crate) handlers: HashMap<String, BoxedHandler>,
}

impl Webview {
    /// Creates a webview from a full inline HTML document.
    pub fn inline(html: impl Into<String>) -> Self {
        Self {
            kind: RegisterKind::Inline {
                html: html.into(),
                interactive: true,
            },
            handlers: HashMap::new(),
        }
    }

    /// Creates a webview served from a directory of assets.
    pub fn dir(root: impl AsRef<Path>, entry: impl Into<String>) -> Self {
        Self {
            kind: RegisterKind::Dir {
                root: root.as_ref().display().to_string(),
                entry: entry.into(),
                interactive: true,
            },
            handlers: HashMap::new(),
        }
    }

    /// Sets the control-plane `interactive` flag (focus/input). Fixed at register.
    pub fn interactive(mut self, interactive: bool) -> Self {
        match &mut self.kind {
            RegisterKind::Inline { interactive: i, .. } => *i = interactive,
            RegisterKind::Dir { interactive: i, .. } => *i = interactive,
        }
        self
    }

    /// Registers an RPC handler for `method`. The parameter is a tuple
    /// deserialized from the page's `window.ozmux.call(method, args)` array.
    ///
    /// # Panics
    /// Panics if `method` starts with the reserved `__ozma.` prefix, which is
    /// owned by the SDK's focus glue.
    pub fn on<P, R, F>(self, method: impl Into<String>, f: F) -> Self
    where
        P: DeserializeOwned,
        R: Serialize,
        F: Fn(P) -> Result<R, crate::error::RpcError> + Send + Sync + 'static,
    {
        let method = method.into();
        assert!(
            !method.starts_with("__ozma."),
            "method {method:?} uses the reserved __ozma. namespace"
        );
        self.on_reserved(method, f)
    }

    pub(crate) fn on_reserved<P, R, F>(mut self, method: impl Into<String>, f: F) -> Self
    where
        P: DeserializeOwned,
        R: Serialize,
        F: Fn(P) -> Result<R, crate::error::RpcError> + Send + Sync + 'static,
    {
        self.handlers.insert(method.into(), make_handler(f));
        self
    }

    #[cfg(test)]
    pub(crate) fn handlers_for_test(
        &self,
    ) -> &std::collections::HashMap<String, crate::handler::BoxedHandler> {
        &self.handlers
    }
}

/// A registered webview handle: emit events to the page, read its id.
#[derive(Clone, Debug)]
pub struct WebviewHandle {
    id: String,
    writer: SharedWriter,
}

impl PartialEq for WebviewHandle {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl WebviewHandle {
    pub(crate) fn new(id: String, writer: SharedWriter) -> Self {
        Self { id, writer }
    }

    /// Returns the opaque handle id minted by the control plane.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Pushes an event to the currently-mounted page(s) of this handle.
    ///
    /// Mount-scoped: a no-op (still `Ok`) when nothing is mounted.
    pub fn emit<T: Serialize>(&self, event: &str, payload: &T) -> OzmaResult<()> {
        let msg = ClientMsg::Emit {
            handle: self.id.clone(),
            event: event.to_owned(),
            payload: serde_json::to_value(payload)?,
        };
        let line = serde_json::to_string(&msg)?;
        let mut w = self.writer.lock()?;
        writeln!(w, "{line}")?;
        w.flush()?;
        Ok(())
    }

    /// Pushes the navigation keymap to this handle's page glue.
    ///
    /// Wraps [`emit`](Self::emit) of the reserved `__ozma.keys` event so the
    /// glue intercepts the same chord the app matches natively. Push it once the
    /// page is mounted (e.g. when focus first reaches the webview).
    pub fn set_nav_keys(&self, keymap: &NavKeymap) -> OzmaResult<()> {
        self.emit("__ozma.keys", keymap)
    }

    /// Tells the page whether the app currently considers it focused.
    ///
    /// Wraps [`emit`](Self::emit) of the reserved `__ozma.focus-state` event;
    /// the page observes it with `window.ozmux.on('__ozma.focus-state', cb)`
    /// (e.g. to focus an input or draw a ring). This app→page notification is
    /// distinct from the page→app `__ozma.focus` report the glue sends.
    pub fn set_page_focus(&self, focused: bool) -> OzmaResult<()> {
        self.emit("__ozma.focus-state", &focused)
    }

    /// Requests host focus on this webview (default instance).
    ///
    /// The host sets `FocusedWebview` to this handle's mounted inline webview;
    /// keystrokes then reach the page natively until the app blurs or moves
    /// focus.
    pub fn focus(&self) -> OzmaResult<()> {
        self.send_focus(Some(self.id.clone()), None)
    }

    /// Requests host focus on a named mount instance of this webview.
    pub fn focus_instance(&self, instance: &str) -> OzmaResult<()> {
        self.send_focus(Some(self.id.clone()), Some(instance.to_owned()))
    }

    fn send_focus(&self, handle: Option<String>, instance: Option<String>) -> OzmaResult<()> {
        let line = serde_json::to_string(&ClientMsg::Focus { handle, instance })?;
        let mut w = self.writer.lock()?;
        writeln!(w, "{line}")?;
        w.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn inline_builder_records_kind_and_default_interactive() {
        let wv = Webview::inline("<h1>hi</h1>");
        match &wv.kind {
            RegisterKind::Inline { html, interactive } => {
                assert_eq!(html, "<h1>hi</h1>");
                assert!(*interactive);
            }
            _ => panic!("expected inline"),
        }
    }

    #[test]
    fn dir_builder_and_non_interactive() {
        let wv = Webview::dir("/abs/ui", "index.html").interactive(false);
        match &wv.kind {
            RegisterKind::Dir {
                root,
                entry,
                interactive,
            } => {
                assert_eq!(root, "/abs/ui");
                assert_eq!(entry, "index.html");
                assert!(!*interactive);
            }
            _ => panic!("expected dir"),
        }
    }

    #[test]
    fn on_registers_handler() {
        let wv = Webview::inline("x").on("ping", |(n,): (String,)| Ok(format!("pong:{n}")));
        let h = wv.handlers.get("ping").expect("handler present");
        assert_eq!(h(vec![json!("hi")]).unwrap(), json!("pong:hi"));
    }

    #[test]
    #[should_panic(expected = "__ozma.")]
    fn user_on_rejects_reserved_namespace() {
        let _ = Webview::inline("x").on("__ozma.nav", |(): ()| Ok::<_, crate::error::RpcError>(()));
    }

    #[test]
    fn on_reserved_installs_handler_under_reserved_name() {
        let wv = Webview::inline("x").on_reserved("__ozma.nav", |(d,): (String,)| {
            Ok::<_, crate::error::RpcError>(format!("nav:{d}"))
        });
        let h = wv
            .handlers
            .get("__ozma.nav")
            .expect("reserved handler present");
        assert_eq!(
            h(vec![serde_json::json!("right")]).unwrap(),
            serde_json::json!("nav:right")
        );
    }

    #[test]
    fn focus_writes_focus_op_line() {
        use std::io::{BufRead, BufReader};
        use std::os::unix::net::UnixStream;
        let (a, b) = UnixStream::pair().unwrap();
        let writer = std::sync::Arc::new(std::sync::Mutex::new(a));
        let handle = WebviewHandle::new("view-1".to_owned(), writer);
        handle.focus().unwrap();
        let mut line = String::new();
        BufReader::new(b).read_line(&mut line).unwrap();
        let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(v["op"], "focus");
        assert_eq!(v["handle"], "view-1");
    }

    fn pair_handle() -> (WebviewHandle, std::os::unix::net::UnixStream) {
        use std::os::unix::net::UnixStream;
        let (a, b) = UnixStream::pair().unwrap();
        let writer = std::sync::Arc::new(std::sync::Mutex::new(a));
        (WebviewHandle::new("v".to_owned(), writer), b)
    }

    fn read_line_value(stream: std::os::unix::net::UnixStream) -> serde_json::Value {
        use std::io::{BufRead, BufReader};
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).unwrap();
        serde_json::from_str(line.trim()).unwrap()
    }

    #[test]
    fn set_nav_keys_emits_ozma_keys_event() {
        let (handle, peer) = pair_handle();
        handle.set_nav_keys(&crate::NavKeymap::arrows()).unwrap();
        let v = read_line_value(peer);
        assert_eq!(v["op"], "emit");
        assert_eq!(v["event"], "__ozma.keys");
        assert_eq!(v["payload"]["keys"]["arrowleft"], "left");
    }

    #[test]
    fn set_page_focus_emits_focus_state_event() {
        let (handle, peer) = pair_handle();
        handle.set_page_focus(true).unwrap();
        let v = read_line_value(peer);
        assert_eq!(v["op"], "emit");
        assert_eq!(v["event"], "__ozma.focus-state");
        assert_eq!(v["payload"], true);
    }
}
