//! Webview builder and registered handle.

use crate::error::OzmaResult;
use crate::handler::{BoxedHandler, make_handler};
use crate::keychord::KeyChord;
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
                passthrough: Vec::new(),
            },
            handlers: HashMap::new(),
        }
    }

    /// Creates a webview that loads a remote `http(s)` URL.
    ///
    /// Display-only by default — the `window.ozmux` back-channel is **not**
    /// injected. Call [`Webview::bridge`] (or register a handler with
    /// [`Webview::on`], which enables it implicitly) to opt in.
    pub fn url(url: impl Into<String>) -> Self {
        Self {
            kind: RegisterKind::Url {
                url: url.into(),
                interactive: true,
                bridge: false,
                passthrough: Vec::new(),
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
                passthrough: Vec::new(),
            },
            handlers: HashMap::new(),
        }
    }

    /// Sets the control-plane `interactive` flag (focus/input). Fixed at register.
    pub fn interactive(mut self, interactive: bool) -> Self {
        match &mut self.kind {
            RegisterKind::Inline { interactive: i, .. } => *i = interactive,
            RegisterKind::Dir { interactive: i, .. } => *i = interactive,
            RegisterKind::Url { interactive: i, .. } => *i = interactive,
        }
        self
    }

    /// Opts a `url` webview into the `window.ozmux` back-channel. A no-op for
    /// `inline`/`dir` webviews, which are always bridged. Fixed at register.
    pub fn bridge(mut self, bridge: bool) -> Self {
        if let RegisterKind::Url { bridge: b, .. } = &mut self.kind {
            *b = bridge;
        }
        self
    }

    /// Declares chords the page lets through to the app while focused (the host
    /// forwards them to the PTY so the app reads them via `crossterm::event::read`).
    pub fn passthrough(mut self, keys: impl IntoIterator<Item = KeyChord>) -> Self {
        match &mut self.kind {
            RegisterKind::Inline { passthrough, .. }
            | RegisterKind::Dir { passthrough, .. }
            | RegisterKind::Url { passthrough, .. } => passthrough.extend(keys),
        }
        self
    }

    /// Registers an RPC handler for `method`. The parameter is a tuple
    /// deserialized from the page's `window.ozmux.call(method, args)` array.
    ///
    /// # Panics
    /// Panics if `method` starts with the reserved `__ozma.` prefix, which is
    /// owned by the SDK.
    pub fn on<P, R, F>(mut self, method: impl Into<String>, f: F) -> Self
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
        self.handlers.insert(method, make_handler(f));
        if let RegisterKind::Url { bridge, .. } = &mut self.kind {
            *bridge = true;
        }
        self
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn inline_builder_records_kind_and_default_interactive() {
        let wv = Webview::inline("<h1>hi</h1>");
        match &wv.kind {
            RegisterKind::Inline {
                html, interactive, ..
            } => {
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
                ..
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
    fn passthrough_rides_register_wire() {
        use ratatui::crossterm::event::{KeyCode, KeyModifiers};
        let wv = Webview::inline("x").passthrough([KeyChord {
            mods: KeyModifiers::ALT,
            code: KeyCode::Char('h'),
        }]);
        let v = serde_json::to_value(crate::protocol::ClientMsg::Register(wv.kind)).unwrap();
        assert_eq!(v["op"], "register");
        assert_eq!(v["passthrough"][0]["key"], "h");
        assert_eq!(v["passthrough"][0]["mods"][0], "alt");
    }

    #[test]
    fn empty_passthrough_is_omitted_from_wire() {
        let wv = Webview::inline("x");
        let v = serde_json::to_value(crate::protocol::ClientMsg::Register(wv.kind)).unwrap();
        assert!(
            v.get("passthrough").is_none(),
            "empty passthrough must be skipped"
        );
    }

    #[test]
    fn url_builder_records_kind_with_display_only_defaults() {
        let wv = Webview::url("https://example.com");
        match &wv.kind {
            RegisterKind::Url {
                url,
                interactive,
                bridge,
                ..
            } => {
                assert_eq!(url, "https://example.com");
                assert!(*interactive, "url webviews are interactive by default");
                assert!(!*bridge, "url webviews are display-only by default");
            }
            _ => panic!("expected url"),
        }
    }

    #[test]
    fn bridge_opts_a_url_webview_into_the_back_channel() {
        let wv = Webview::url("https://example.com").bridge(true);
        match &wv.kind {
            RegisterKind::Url { bridge, .. } => assert!(*bridge),
            _ => panic!("expected url"),
        }
    }

    #[test]
    fn on_implicitly_enables_the_bridge_for_url_webviews() {
        let wv = Webview::url("https://example.com")
            .on("ping", |(): ()| Ok::<_, crate::error::RpcError>(()));
        match &wv.kind {
            RegisterKind::Url { bridge, .. } => {
                assert!(*bridge, "registering a handler must enable the bridge");
            }
            _ => panic!("expected url"),
        }
    }

    #[test]
    fn url_passthrough_rides_register_wire() {
        use ratatui::crossterm::event::{KeyCode, KeyModifiers};
        let wv = Webview::url("https://example.com").passthrough([KeyChord {
            mods: KeyModifiers::ALT,
            code: KeyCode::Char('h'),
        }]);
        let v = serde_json::to_value(crate::protocol::ClientMsg::Register(wv.kind)).unwrap();
        assert_eq!(v["kind"], "url");
        assert_eq!(v["passthrough"][0]["key"], "h");
    }
}
