//! Webview builder and registered handle.

use crate::error::OrzmaResult;
use crate::events::{EventDecl, EventQueues};
use crate::handler::{BoxedHandler, make_handler};
use crate::keychord::KeyChord;
use crate::protocol::{ClientMsg, HandleId, NavAction, RegisterKind};
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
    /// Display-only by default — the `window.orzma` back-channel is **not**
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

    /// Opts a `url` webview into the `window.orzma` back-channel. A no-op for
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
    /// order supplied. Runs after the host's `window.orzma` bridge, so a script
    /// may use `window.orzma` when the view is bridged. Additive across calls;
    /// applies to all view kinds, including display-only `url` views.
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
    /// page passes to `window.orzma.call(method, params)` (an object becomes a
    /// struct, an array a tuple, and an omitted/`null` params the unit `()`).
    ///
    /// # Panics
    /// Panics if `method` starts with the reserved `__orzma.` prefix, which is
    /// owned by the SDK.
    pub fn on<P, R, F>(mut self, method: impl Into<String>, f: F) -> Self
    where
        P: DeserializeOwned,
        R: Serialize,
        F: Fn(P) -> Result<R, crate::error::RpcError> + Send + Sync + 'static,
    {
        let method = method.into();
        assert!(
            !method.starts_with("__orzma."),
            "method {method:?} uses the reserved __orzma. namespace"
        );
        self.handlers.insert(method, make_handler(f));
        self.enable_bridge_for_url();
        self
    }

    /// Declares an inbound event the page may send via `window.orzma.emit(name, …)`,
    /// binding the wire `name` to the Rust type `T`. The app later drains it with
    /// [`WebviewHandle::read_events::<T>`]. Enables the `window.orzma` bridge for
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

    /// Force-enables the `window.orzma` bridge for a `url` webview; a no-op for
    /// `inline`/`dir`, which are always bridged. Shared by `on` and `add_event`,
    /// both of which require the bridge for the page-side channel they wire.
    fn enable_bridge_for_url(&mut self) {
        if let RegisterKind::Url { bridge, .. } = &mut self.kind {
            *bridge = true;
        }
    }
}

/// A registered webview: emit events to its page(s), drive its default
/// placement, and read the two ids it is addressed by.
///
/// A registration is addressed by a [`HandleId`], and each of its placements by
/// an instance id. [`WebviewHandle::instance_id`] returns the default
/// placement's; extra placements come from [`crate::Orzma::new_instance`].
#[derive(Clone, Debug)]
pub struct WebviewHandle {
    handle: Arc<Mutex<HandleId>>,
    instance: Arc<Mutex<String>>,
    events: Arc<EventQueues>,
    writer: SharedWriter,
}

impl PartialEq for WebviewHandle {
    fn eq(&self, other: &Self) -> bool {
        self.handle_id() == other.handle_id()
    }
}

impl WebviewHandle {
    /// Returns the opaque handle the control plane minted for this
    /// registration.
    ///
    /// This addresses the registration itself, which is what
    /// [`crate::Orzma::new_instance`] mints from. It is not what a placement is
    /// mounted or navigated by: pass [`WebviewHandle::instance_id`] to
    /// [`crate::WebviewWidget::new`].
    pub fn handle_id(&self) -> HandleId {
        self.handle
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Returns the id of this registration's default placement.
    ///
    /// This is the id [`crate::WebviewWidget::new`] mounts and the navigation
    /// methods below drive.
    pub fn instance_id(&self) -> String {
        self.instance
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Pushes an event to every currently-mounted page of this registration.
    ///
    /// Mount-scoped: a no-op (still `Ok`) when nothing is mounted.
    pub fn emit<T: Serialize>(&self, event: &str, payload: &T) -> OrzmaResult<()> {
        let msg = ClientMsg::Emit {
            handle: self.handle_id(),
            event: event.to_owned(),
            payload: serde_json::to_value(payload)?,
        };
        let line = serde_json::to_string(&msg)?;
        let mut w = self.writer.lock()?;
        writeln!(w, "{line}")?;
        w.flush()?;
        Ok(())
    }

    /// Navigates this registration's default placement to `url` in place (no
    /// re-registration). Mount-scoped: a no-op (still `Ok`) when nothing is
    /// mounted under that instance. Other placements are left where they are.
    pub fn navigate(&self, url: impl Into<String>) -> OrzmaResult<()> {
        send_nav(&self.writer, self.instance_id(), NavAction::To(url.into()))
    }

    /// Goes back in the default placement's native session history.
    /// Mount-scoped; other placements are unaffected.
    pub fn go_back(&self) -> OrzmaResult<()> {
        send_nav(&self.writer, self.instance_id(), NavAction::Back)
    }

    /// Goes forward in the default placement's native session history.
    /// Mount-scoped; other placements are unaffected.
    pub fn go_forward(&self) -> OrzmaResult<()> {
        send_nav(&self.writer, self.instance_id(), NavAction::Forward)
    }

    /// Reloads the default placement's page. Mount-scoped; other placements are
    /// unaffected.
    pub fn reload(&self) -> OrzmaResult<()> {
        send_nav(&self.writer, self.instance_id(), NavAction::Reload)
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

    /// Whether `slot` is the very slot this handle reads its handle id from.
    ///
    /// Registrations are matched against a handle by this identity rather than
    /// by the id the slot currently holds: a reconnect refills the slot in
    /// place, so an id read before a round trip can no longer be found by value
    /// once the replay lands.
    pub(crate) fn shares_handle_slot(&self, slot: &Arc<Mutex<HandleId>>) -> bool {
        Arc::ptr_eq(&self.handle, slot)
    }

    /// Creates a handle over pre-existing shared id slots, which the reconnect
    /// replay refills in place.
    pub(crate) fn new_shared(
        handle: Arc<Mutex<HandleId>>,
        instance: Arc<Mutex<String>>,
        events: Arc<EventQueues>,
        writer: SharedWriter,
    ) -> Self {
        Self {
            handle,
            instance,
            events,
            writer,
        }
    }
}

/// One extra placement of a registration, minted by
/// [`crate::Orzma::new_instance`].
///
/// It can be mounted at the same time as the registration's default placement
/// and as its other extra placements, each showing the same content
/// independently.
#[derive(Clone, Debug)]
pub struct WebviewInstance {
    instance: Arc<Mutex<String>>,
    writer: SharedWriter,
}

impl WebviewInstance {
    /// Returns the id of this placement, to pass to
    /// [`crate::WebviewWidget::new`].
    pub fn id(&self) -> String {
        self.instance
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Navigates this placement to `url` in place. Mount-scoped: a no-op (still
    /// `Ok`) when nothing is mounted under this instance.
    pub fn navigate(&self, url: impl Into<String>) -> OrzmaResult<()> {
        send_nav(&self.writer, self.id(), NavAction::To(url.into()))
    }

    /// Goes back in this placement's native session history. Mount-scoped.
    pub fn go_back(&self) -> OrzmaResult<()> {
        send_nav(&self.writer, self.id(), NavAction::Back)
    }

    /// Goes forward in this placement's native session history. Mount-scoped.
    pub fn go_forward(&self) -> OrzmaResult<()> {
        send_nav(&self.writer, self.id(), NavAction::Forward)
    }

    /// Reloads this placement's page. Mount-scoped.
    pub fn reload(&self) -> OrzmaResult<()> {
        send_nav(&self.writer, self.id(), NavAction::Reload)
    }

    /// Creates a placement over a pre-existing shared instance slot, which the
    /// reconnect replay refills in place.
    pub(crate) fn new_shared(instance: Arc<Mutex<String>>, writer: SharedWriter) -> Self {
        Self { instance, writer }
    }
}

/// Writes one `navigate` op addressed to `instance`.
fn send_nav(writer: &SharedWriter, instance: String, action: NavAction) -> OrzmaResult<()> {
    let msg = ClientMsg::Navigate { instance, action };
    let line = serde_json::to_string(&msg)?;
    let mut w = writer.lock()?;
    writeln!(w, "{line}")?;
    w.flush()?;
    Ok(())
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
    #[should_panic(expected = "__orzma.")]
    fn user_on_rejects_reserved_namespace() {
        let _ = Webview::inline("x").on("__orzma.nav", |_: ()| Ok::<_, crate::error::RpcError>(()));
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

    /// Asserts that both ids a handle exposes are read through their shared
    /// slots, so a refill is visible to a handle already in a caller's hands.
    ///
    /// Case: the session reconnects and re-registers, which mints a fresh
    /// handle and instance for a view the app is still holding and drawing.
    #[test]
    fn both_ids_reflect_a_slot_update() {
        let handle_slot = Arc::new(Mutex::new(HandleId::from("old-handle".to_owned())));
        let instance_slot = Arc::new(Mutex::new("old-instance".to_owned()));
        let (a, _b) = std::os::unix::net::UnixStream::pair().unwrap();
        let writer: SharedWriter = Arc::new(Mutex::new(a));
        let handle = WebviewHandle::new_shared(
            handle_slot.clone(),
            instance_slot.clone(),
            Arc::new(crate::events::EventQueues::from_decls(&[])),
            writer,
        );
        assert_eq!(handle.handle_id().to_string(), "old-handle");
        assert_eq!(handle.instance_id(), "old-instance");

        *handle_slot.lock().unwrap() = HandleId::from("new-handle".to_owned());
        *instance_slot.lock().unwrap() = "new-instance".to_owned();
        assert_eq!(handle.handle_id().to_string(), "new-handle");
        assert_eq!(handle.instance_id(), "new-instance");
    }

    /// Asserts that a navigation names the placement it drives, so a handle
    /// with several placements moves only its default one.
    ///
    /// Case: the app follows a link in the pane showing a view's default
    /// placement while a second placement of that view stays where it is.
    #[test]
    fn a_handle_navigation_addresses_its_default_instance() {
        use std::io::{BufRead, BufReader};
        let (client, server) = std::os::unix::net::UnixStream::pair().unwrap();
        let writer: SharedWriter = Arc::new(Mutex::new(client));
        let handle = WebviewHandle::new_shared(
            Arc::new(Mutex::new(HandleId::from("h".to_owned()))),
            Arc::new(Mutex::new("i1".to_owned())),
            Arc::new(crate::events::EventQueues::from_decls(&[])),
            writer,
        );

        handle.navigate("https://example.com").unwrap();

        let mut line = String::new();
        BufReader::new(server).read_line(&mut line).unwrap();
        let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(v["op"], "navigate");
        assert_eq!(v["instance"], "i1");
        assert_eq!(v["action"]["to"], "https://example.com");
    }

    /// Asserts that a minted placement drives its own instance rather than the
    /// registration's default one.
    ///
    /// Case: an app reloads the right-hand pane of a split showing one view
    /// twice, leaving the left-hand pane untouched.
    #[test]
    fn an_extra_instance_navigates_itself() {
        use std::io::{BufRead, BufReader};
        let (client, server) = std::os::unix::net::UnixStream::pair().unwrap();
        let writer: SharedWriter = Arc::new(Mutex::new(client));
        let instance = WebviewInstance::new_shared(Arc::new(Mutex::new("i2".to_owned())), writer);

        assert_eq!(instance.id(), "i2");
        instance.reload().unwrap();

        let mut line = String::new();
        BufReader::new(server).read_line(&mut line).unwrap();
        let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(v["op"], "navigate");
        assert_eq!(v["instance"], "i2");
        assert_eq!(v["action"], "reload");
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
        let handle = WebviewHandle::new_shared(
            Arc::new(Mutex::new(HandleId::from("h".to_owned()))),
            Arc::new(Mutex::new("i1".to_owned())),
            events,
            writer,
        );

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
