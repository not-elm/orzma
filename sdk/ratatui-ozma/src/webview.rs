//! Webview builder and registered handle.

use crate::error::OzmaResult;
use crate::events::{EventDecl, EventQueues};
use crate::handler::{BoxedHandler, make_handler};
use crate::keychord::KeyChord;
use crate::protocol::{ClientMsg, NavAction, RegisterKind};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::any::TypeId;
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
    pub(crate) event_decls: Vec<EventDecl>,
}

impl Webview {
    /// Creates a webview from a full inline HTML document.
    pub fn inline(html: impl Into<String>) -> Self {
        Self {
            kind: RegisterKind::Inline {
                html: html.into(),
                interactive: true,
                forward_keys: Vec::new(),
                preload: Vec::new(),
            },
            handlers: HashMap::new(),
            event_decls: Vec::new(),
        }
    }

    /// Creates a webview that loads a remote `http(s)` URL.
    ///
    /// Display-only by default — the `window.ozma` back-channel is **not**
    /// injected. Call [`Webview::bridge`] (or register a handler with
    /// [`Webview::on`], which enables it implicitly) to opt in.
    pub fn url(url: impl Into<String>) -> Self {
        Self {
            kind: RegisterKind::Url {
                url: url.into(),
                interactive: true,
                bridge: false,
                forward_keys: Vec::new(),
                preload: Vec::new(),
            },
            handlers: HashMap::new(),
            event_decls: Vec::new(),
        }
    }

    /// Creates a webview served from a directory of assets.
    pub fn dir(root: impl AsRef<Path>, entry: impl Into<String>) -> Self {
        Self {
            kind: RegisterKind::Dir {
                root: root.as_ref().display().to_string(),
                entry: entry.into(),
                interactive: true,
                forward_keys: Vec::new(),
                preload: Vec::new(),
            },
            handlers: HashMap::new(),
            event_decls: Vec::new(),
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

    /// Opts a `url` webview into the `window.ozma` back-channel. A no-op for
    /// `inline`/`dir` webviews, which are always bridged. Fixed at register.
    pub fn bridge(mut self, bridge: bool) -> Self {
        if let RegisterKind::Url { bridge: b, .. } = &mut self.kind {
            *b = bridge;
        }
        self
    }

    /// Declares chords the page lets through to the app while focused (the host
    /// forwards them to the PTY so the app reads them via `crossterm::event::read`).
    pub fn forward_keys(mut self, keys: impl IntoIterator<Item = KeyChord>) -> Self {
        match &mut self.kind {
            RegisterKind::Inline { forward_keys, .. }
            | RegisterKind::Dir { forward_keys, .. }
            | RegisterKind::Url { forward_keys, .. } => forward_keys.extend(keys),
        }
        self
    }

    /// Declares JavaScript injected before the page's own scripts run, in the
    /// order supplied. Runs after the host's `window.ozma` bridge (and the
    /// link-hint engine for url views), so a script may use `window.ozma` when
    /// the view is bridged. Additive across calls; applies to all view kinds,
    /// including display-only `url` views.
    ///
    /// Each entry should be a complete, self-contained statement: the host
    /// concatenates all preload scripts with `;` and evaluates them as one
    /// script in the page's shared context, so a trailing `//` line comment, a
    /// top-level redeclaration colliding with the bridge's identifiers, or a
    /// syntax error can break the whole eval. Wrapping each entry in an IIFE
    /// (`(() => { … })();`) is the safe idiom.
    pub fn preload(mut self, scripts: impl IntoIterator<Item = impl Into<String>>) -> Self {
        match &mut self.kind {
            RegisterKind::Inline { preload, .. }
            | RegisterKind::Dir { preload, .. }
            | RegisterKind::Url { preload, .. } => {
                preload.extend(scripts.into_iter().map(Into::into));
            }
        }
        self
    }

    /// Registers an RPC handler for `method`. The parameter is any
    /// `DeserializeOwned` type, deserialized from the single `params` value the
    /// page passes to `window.ozma.call(method, params)` (an object becomes a
    /// struct, an array a tuple, and an omitted/`null` params the unit `()`).
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
        self.enable_bridge_for_url();
        self
    }

    /// Declares an inbound event the page may send via `window.ozma.emit(name, …)`,
    /// binding the wire `name` to the Rust type `T`. The app later drains it with
    /// [`WebviewHandle::read_events::<T>`]. Enables the `window.ozma` bridge for
    /// `url` webviews (like [`Webview::on`]); a no-op for `inline`/`dir`, which
    /// are always bridged.
    ///
    /// # Panics
    /// Panics if `name` or the type `T` is already registered on this builder —
    /// the type ↔ name mapping must be 1:1. (`on` silently overwrites a
    /// duplicate method; `add_event` enforces uniqueness instead.)
    pub fn add_event<T: DeserializeOwned + 'static>(mut self, name: impl Into<String>) -> Self {
        let name = name.into();
        let type_id = TypeId::of::<T>();
        assert!(
            !self.event_decls.iter().any(|d| d.name == name),
            "event name {name:?} is already registered"
        );
        assert!(
            !self.event_decls.iter().any(|d| d.type_id == type_id),
            "event type {} is already registered",
            std::any::type_name::<T>()
        );
        self.event_decls.push(EventDecl { name, type_id });
        self.enable_bridge_for_url();
        self
    }

    /// Force-enables the `window.ozma` bridge for a `url` webview; a no-op for
    /// `inline`/`dir`, which are always bridged. Shared by `on` and `add_event`,
    /// both of which require the bridge for the page-side channel they wire.
    fn enable_bridge_for_url(&mut self) {
        if let RegisterKind::Url { bridge, .. } = &mut self.kind {
            *bridge = true;
        }
    }
}

/// A registered webview handle: emit events to the page, read its id.
#[derive(Clone, Debug)]
pub struct WebviewHandle {
    id: Arc<Mutex<String>>,
    events: Arc<EventQueues>,
    writer: SharedWriter,
}

impl PartialEq for WebviewHandle {
    fn eq(&self, other: &Self) -> bool {
        self.id() == other.id()
    }
}

impl WebviewHandle {
    /// Returns the opaque handle id minted by the control plane.
    pub fn id(&self) -> String {
        self.id.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Pushes an event to the currently-mounted page(s) of this handle.
    ///
    /// Mount-scoped: a no-op (still `Ok`) when nothing is mounted.
    pub fn emit<T: Serialize>(&self, event: &str, payload: &T) -> OzmaResult<()> {
        let msg = ClientMsg::Emit {
            handle: self.id(),
            event: event.to_owned(),
            payload: serde_json::to_value(payload)?,
        };
        let line = serde_json::to_string(&msg)?;
        let mut w = self.writer.lock()?;
        writeln!(w, "{line}")?;
        w.flush()?;
        Ok(())
    }

    /// Navigates this handle's mounted webview to `url` in place (no
    /// re-registration). Mount-scoped: a no-op (still `Ok`) when nothing is
    /// mounted.
    pub fn navigate(&self, url: impl Into<String>) -> OzmaResult<()> {
        self.send_nav(NavAction::To(url.into()))
    }

    /// Goes back in the webview's native session history. Mount-scoped.
    pub fn go_back(&self) -> OzmaResult<()> {
        self.send_nav(NavAction::Back)
    }

    /// Goes forward in the webview's native session history. Mount-scoped.
    pub fn go_forward(&self) -> OzmaResult<()> {
        self.send_nav(NavAction::Forward)
    }

    /// Reloads the current page. Mount-scoped.
    pub fn reload(&self) -> OzmaResult<()> {
        self.send_nav(NavAction::Reload)
    }

    /// Drains and returns every buffered event of type `T`, oldest first.
    /// Payloads that fail to deserialize into `T` are dropped and logged; the
    /// result is empty if `T` was never declared via [`Webview::add_event`].
    pub fn read_events<T: DeserializeOwned + 'static>(&self) -> Vec<T> {
        self.events
            .drain_type(TypeId::of::<T>())
            .into_iter()
            .filter_map(|v| match serde_json::from_value::<T>(v) {
                Ok(t) => Some(t),
                Err(e) => {
                    tracing::warn!(error = %e, "dropping inbound event that failed to deserialize");
                    None
                }
            })
            .collect()
    }

    /// Creates a handle from a pre-existing shared ID slot for callers that need
    /// to share the ID slot across threads.
    pub(crate) fn new_shared(
        id: Arc<Mutex<String>>,
        events: Arc<EventQueues>,
        writer: SharedWriter,
    ) -> Self {
        Self { id, events, writer }
    }

    fn send_nav(&self, action: NavAction) -> OzmaResult<()> {
        let msg = ClientMsg::Navigate {
            handle: self.id(),
            action,
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
        let wv = Webview::inline("x").on("ping", |n: String| Ok(format!("pong:{n}")));
        let h = wv.handlers.get("ping").expect("handler present");
        assert_eq!(h(json!("hi")).unwrap(), json!("pong:hi"));
    }

    #[test]
    #[should_panic(expected = "__ozma.")]
    fn user_on_rejects_reserved_namespace() {
        let _ = Webview::inline("x").on("__ozma.nav", |_: ()| Ok::<_, crate::error::RpcError>(()));
    }

    #[test]
    fn forward_keys_rides_register_wire() {
        use ratatui::crossterm::event::{KeyCode, KeyModifiers};
        let wv = Webview::inline("x").forward_keys([KeyChord {
            mods: KeyModifiers::ALT,
            code: KeyCode::Char('h'),
        }]);
        let v = serde_json::to_value(crate::protocol::ClientMsg::Register(wv.kind)).unwrap();
        assert_eq!(v["op"], "register");
        assert_eq!(v["forward_keys"][0]["key"], "h");
        assert_eq!(v["forward_keys"][0]["mods"][0], "alt");
    }

    #[test]
    fn empty_forward_keys_is_omitted_from_wire() {
        let wv = Webview::inline("x");
        let v = serde_json::to_value(crate::protocol::ClientMsg::Register(wv.kind)).unwrap();
        assert!(
            v.get("forward_keys").is_none(),
            "empty forward_keys must be skipped"
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
    fn url_forward_keys_rides_register_wire() {
        use ratatui::crossterm::event::{KeyCode, KeyModifiers};
        let wv = Webview::url("https://example.com").forward_keys([KeyChord {
            mods: KeyModifiers::ALT,
            code: KeyCode::Char('h'),
        }]);
        let v = serde_json::to_value(crate::protocol::ClientMsg::Register(wv.kind)).unwrap();
        assert_eq!(v["kind"], "url");
        assert_eq!(v["forward_keys"][0]["key"], "h");
    }

    #[test]
    fn preload_accumulates_across_calls_and_rides_register_wire() {
        let wv = Webview::inline("x")
            .preload(["window.A = 1;"])
            .preload(["window.B = 2;"]);
        let v = serde_json::to_value(crate::protocol::ClientMsg::Register(wv.kind)).unwrap();
        assert_eq!(v["preload"][0], "window.A = 1;");
        assert_eq!(v["preload"][1], "window.B = 2;");
    }

    #[test]
    fn preload_rides_wire_for_every_kind() {
        for wv in [
            Webview::inline("x").preload(["a"]),
            Webview::dir("/abs/ui", "index.html").preload(["a"]),
            Webview::url("https://example.com").preload(["a"]),
        ] {
            let v = serde_json::to_value(crate::protocol::ClientMsg::Register(wv.kind)).unwrap();
            assert_eq!(
                v["preload"][0], "a",
                "preload must ride the wire for every kind"
            );
        }
    }

    #[test]
    fn empty_preload_is_omitted_from_wire() {
        let wv = Webview::inline("x");
        let v = serde_json::to_value(crate::protocol::ClientMsg::Register(wv.kind)).unwrap();
        assert!(v.get("preload").is_none(), "empty preload must be skipped");
    }

    #[test]
    fn id_reflects_slot_update() {
        let slot = Arc::new(Mutex::new("old-id".to_owned()));
        let (a, _b) = std::os::unix::net::UnixStream::pair().unwrap();
        let writer: SharedWriter = Arc::new(Mutex::new(a));
        let handle = WebviewHandle::new_shared(
            slot.clone(),
            Arc::new(crate::events::EventQueues::from_decls(&[])),
            writer,
        );
        assert_eq!(handle.id(), "old-id");
        *slot.lock().unwrap() = "new-id".to_owned();
        assert_eq!(handle.id(), "new-id");
    }

    #[test]
    fn read_events_drains_and_deserializes_and_skips_bad() {
        use crate::events::{EventDecl, EventQueues};
        #[derive(serde::Deserialize, PartialEq, Debug)]
        struct Hello {
            message: String,
        }
        let decls = vec![EventDecl {
            name: "hello".into(),
            type_id: std::any::TypeId::of::<Hello>(),
        }];
        let events = Arc::new(EventQueues::from_decls(&decls));
        events.ingest("hello", json!({"message": "a"}));
        events.ingest("hello", json!({"nope": 1}));
        events.ingest("hello", json!({"message": "b"}));

        let (sock, _b) = std::os::unix::net::UnixStream::pair().unwrap();
        let writer: SharedWriter = Arc::new(Mutex::new(sock));
        let handle =
            WebviewHandle::new_shared(Arc::new(Mutex::new("h".to_owned())), events, writer);

        let got = handle.read_events::<Hello>();
        assert_eq!(
            got,
            vec![
                Hello {
                    message: "a".into()
                },
                Hello {
                    message: "b".into()
                }
            ]
        );
        // Drained: a second read is empty.
        assert!(handle.read_events::<Hello>().is_empty());
    }

    #[test]
    fn add_event_records_decl() {
        #[derive(serde::Deserialize)]
        struct Hello;
        let wv = Webview::inline("x").add_event::<Hello>("hello");
        assert_eq!(wv.event_decls.len(), 1);
        assert_eq!(wv.event_decls[0].name, "hello");
        assert_eq!(wv.event_decls[0].type_id, std::any::TypeId::of::<Hello>());
    }

    #[test]
    fn add_event_enables_bridge_for_url() {
        #[derive(serde::Deserialize)]
        struct Hello;
        let wv = Webview::url("https://example.com").add_event::<Hello>("hello");
        match &wv.kind {
            RegisterKind::Url { bridge, .. } => assert!(*bridge),
            _ => panic!("expected url"),
        }
    }

    #[test]
    #[should_panic(expected = "already registered")]
    fn add_event_rejects_duplicate_name() {
        #[derive(serde::Deserialize)]
        struct A;
        #[derive(serde::Deserialize)]
        struct B;
        let _ = Webview::inline("x")
            .add_event::<A>("dup")
            .add_event::<B>("dup");
    }

    #[test]
    #[should_panic(expected = "already registered")]
    fn add_event_rejects_duplicate_type() {
        #[derive(serde::Deserialize)]
        struct A;
        let _ = Webview::inline("x")
            .add_event::<A>("one")
            .add_event::<A>("two");
    }
}
