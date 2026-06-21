# ratatui-ozma inbound events (Webview → app) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a one-way, poll-based event channel so a webview page can notify its host ratatui app via `window.ozma.emit(name, payload)` and the app drains a typed queue with `view.read_events::<T>()`.

**Architecture:** Page `emit` → CEF bridge `cef.emit({kind:'ozma.emit',…})` → a new host observer forwards `{"op":"event",…}` over the owning control connection → the SDK reader thread routes the payload into a per-handle bounded ring → `read_events::<T>()` drains and deserializes on the UI thread. It mirrors the existing `call` RPC path but strips reqId/reply/RPC tracking.

**Tech Stack:** Rust (edition 2024, toolchain 1.95, Bevy 0.18, serde/serde_json, tracing), TypeScript (`@ozma/web`, vitest, biome), JS bridge (CEF).

Spec: `docs/superpowers/specs/2026-06-21-ratatui-ozma-inbound-events-design.md`.

## Global Constraints

- **Rust edition 2024, toolchain pinned 1.95.** No `mod.rs` (use `foo.rs` + `foo/`).
- **Comment taxonomy (Rust):** only `// TODO:`, `// NOTE:` (critical caveats only), `// SAFETY:`. All comments in English.
- **Doc comments:** every externally-`pub` item needs `///`; every file-level module needs `//!`. `#![warn(missing_docs)]` is on in `ratatui-ozma`.
- **Imports:** all `use` at the top of the file, one contiguous block, no blank-line grouping, no inline fully-qualified paths in signatures/bodies.
- **Visibility:** narrowest that compiles. SDK-internal items are `pub(crate)`; only the public API (`add_event`, `read_events`) is `pub`.
- **Item ordering:** `pub` items before private; private helpers last. **Parameter ordering:** mutable params before immutable (the `On<E>` trigger stays first).
- **`Plugin::build`:** single method chain off `app.`.
- **TypeScript:** biome-enforced ordering; JSDoc on every `export`; comment taxonomy `// TODO:` / `// NOTE:` / `// biome-ignore` / `// @ts-expect-error` (each with a reason). ECMAScript-erasable only in `ozma.ts` (no enum/namespace/param-properties).
- **Lint gate per task:** `cargo clippy --workspace --all-targets` clean and `cargo fmt` applied for Rust changes; `pnpm lint` + `pnpm check-types` for TS changes.
- **Host-side caveat:** building/testing the root `ozmux-gui` binary (Tasks 6–7) requires CEF provisioned once via `make setup-cef` (macOS). The SDK crate (`ratatui-ozma`) and `@ozma/web` build without CEF.

## File Structure

| File | Responsibility | Tasks |
| --- | --- | --- |
| `sdk/ratatui-ozma/src/events.rs` (new) | `EventQueues` rings: bounded ingest (drop-oldest + throttle), type-keyed drain | 1 |
| `sdk/ratatui-ozma/src/lib.rs` | declare `mod events;` | 1 |
| `sdk/ratatui-ozma/src/protocol.rs` | `IncomingEvent` wire type | 2 |
| `sdk/ratatui-ozma/src/webview.rs` | `Webview::add_event`, `WebviewHandle::read_events` + `events` field | 3, 4 |
| `sdk/ratatui-ozma/src/session.rs` | build/install `EventQueues`, reader `op=="event"` branch, reconnect replay | 4, 5 |
| `src/webview/render/ozma_bridge.js` | page-side `window.ozma.emit` | 6 |
| `src/webview/render/preload.rs` | preload assertion for `ozma.emit` | 6 |
| `src/webview/render.rs` | host `on_ozmux_emit_frame` forwarder | 7 |
| `sdk/ozma-web/src/ozma.ts` + `ozma.test.ts` | `@ozma/web` `emit` client | 8 |
| `sdk/ratatui-ozma/tests/integration.rs` | end-to-end emit → read_events | 9 |
| `sdk/ratatui-ozma/examples/ratatui_webview.rs` | round-trip demo | 10 |

Tasks 1→5 are sequential (SDK core). Tasks 6, 7, 8 are independent of 1–5 and of each other. Task 9 depends on 1–5; Task 10 depends on 1–4.

---

### Task 1: `EventQueues` bounded-ring core

**Files:**
- Create: `sdk/ratatui-ozma/src/events.rs`
- Modify: `sdk/ratatui-ozma/src/lib.rs` (add `mod events;`)
- Test: inline `#[cfg(test)] mod tests` in `events.rs`

**Interfaces:**
- Produces:
  - `pub(crate) struct EventDecl { name: String, type_id: TypeId }` (fields `pub(crate)`)
  - `pub(crate) struct EventQueues` with `fn from_decls(decls: &[EventDecl]) -> Self`, `fn ingest(&self, name: &str, payload: Value) -> bool`, `fn drain_type(&self, type_id: TypeId) -> Vec<Value>`, and `impl Default`
  - `pub(crate) type EventRegistry = Arc<Mutex<HashMap<String, Arc<EventQueues>>>>`

- [ ] **Step 1: Add the module declaration**

In `sdk/ratatui-ozma/src/lib.rs`, add `mod events;` to the module list (alphabetical, after `mod error;`):

```rust
mod backend;
mod error;
mod events;
mod handler;
mod keychord;
mod osc;
mod protocol;
mod session;
mod webview;
mod widget;
```

- [ ] **Step 2: Write the failing tests**

Create `sdk/ratatui-ozma/src/events.rs` with only the tests first (the types don't exist yet, so it won't compile — that is the "red" state for this data-structure task):

```rust
//! Per-handle inbound event queues: bounded rings the reader thread fills from
//! `op == "event"` lines and `WebviewHandle::read_events` drains by type.

use serde_json::Value;
use std::any::TypeId;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct A;
    struct B;

    fn decls() -> Vec<EventDecl> {
        vec![
            EventDecl { name: "a".into(), type_id: TypeId::of::<A>() },
            EventDecl { name: "b".into(), type_id: TypeId::of::<B>() },
        ]
    }

    #[test]
    fn ingest_then_drain_by_type_is_fifo() {
        let q = EventQueues::from_decls(&decls());
        assert!(q.ingest("a", json!(1)));
        assert!(q.ingest("a", json!(2)));
        let drained = q.drain_type(TypeId::of::<A>());
        assert_eq!(drained, vec![json!(1), json!(2)]);
        // A second drain is empty; the ring was consumed.
        assert!(q.drain_type(TypeId::of::<A>()).is_empty());
    }

    #[test]
    fn drain_is_isolated_per_type() {
        let q = EventQueues::from_decls(&decls());
        q.ingest("a", json!("x"));
        q.ingest("b", json!("y"));
        assert_eq!(q.drain_type(TypeId::of::<A>()), vec![json!("x")]);
        assert_eq!(q.drain_type(TypeId::of::<B>()), vec![json!("y")]);
    }

    #[test]
    fn ingest_for_undeclared_name_returns_false() {
        let q = EventQueues::from_decls(&decls());
        assert!(!q.ingest("missing", json!(1)));
    }

    #[test]
    fn drain_for_undeclared_type_is_empty() {
        struct C;
        let q = EventQueues::from_decls(&decls());
        assert!(q.drain_type(TypeId::of::<C>()).is_empty());
    }

    #[test]
    fn overflow_drops_oldest_and_keeps_cap() {
        let q = EventQueues::from_decls_with_cap(&decls(), 2);
        q.ingest("a", json!(1));
        q.ingest("a", json!(2));
        q.ingest("a", json!(3)); // evicts 1
        let drained = q.drain_type(TypeId::of::<A>());
        assert_eq!(drained, vec![json!(2), json!(3)]);
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p ratatui-ozma events::`
Expected: FAIL to compile — `EventDecl`, `EventQueues`, `from_decls`, etc. not found.

- [ ] **Step 4: Write the implementation**

Insert the implementation between the `use` block and the `#[cfg(test)] mod tests` in `events.rs`:

```rust
/// The default per-event ring capacity. Overflow drops the oldest payload.
const DEFAULT_CAP: usize = 1024;

/// Minimum interval between overflow warnings for a single saturated ring.
const WARN_EVERY: Duration = Duration::from_secs(5);

/// A builder-time declaration binding a wire event name to a Rust type.
pub(crate) struct EventDecl {
    /// The wire event name the page sends via `window.ozma.emit`.
    pub(crate) name: String,
    /// `TypeId::of::<T>()` for the declared event type `T`.
    pub(crate) type_id: TypeId,
}

/// One bounded ring of raw payloads plus throttled-overflow bookkeeping.
#[derive(Default, Debug)]
struct RingBuf {
    buf: VecDeque<Value>,
    dropped: u64,
    last_warn: Option<Instant>,
}

type Ring = Arc<Mutex<RingBuf>>;

/// The per-handle set of inbound event rings, declared at `register` and shared
/// between the reader thread (`by_name` ingest) and the `WebviewHandle`
/// (`by_type` drain). Each ring is shared by both maps via one `Arc`, so each
/// side reaches it in a single lookup. Both maps are frozen after construction;
/// only ring contents mutate, so the whole struct is shared behind one `Arc`
/// with no outer lock.
#[derive(Default, Debug)]
pub(crate) struct EventQueues {
    by_name: HashMap<String, Ring>,
    by_type: HashMap<TypeId, Ring>,
    cap: usize,
}

/// Maps a registration handle to its `EventQueues`, the inbound-event peer of
/// the SDK's per-handle handler registry.
pub(crate) type EventRegistry = Arc<Mutex<HashMap<String, Arc<EventQueues>>>>;

impl EventQueues {
    /// Builds the rings for `decls` at the default capacity, inserting each ring
    /// into both lookup maps.
    pub(crate) fn from_decls(decls: &[EventDecl]) -> Self {
        Self::from_decls_with_cap(decls, DEFAULT_CAP)
    }

    /// Routes `payload` into the ring named `name`. When the ring is at
    /// capacity, drops the oldest payload first (with a per-ring throttled
    /// warning). Returns `false` when `name` was never declared.
    pub(crate) fn ingest(&self, name: &str, payload: Value) -> bool {
        let Some(ring) = self.by_name.get(name) else {
            return false;
        };
        let mut ring = ring.lock().unwrap_or_else(|e| e.into_inner());
        if ring.buf.len() >= self.cap {
            ring.buf.pop_front();
            ring.dropped += 1;
            if ring.last_warn.is_none_or(|t| t.elapsed() >= WARN_EVERY) {
                tracing::warn!(
                    event = name,
                    dropped = ring.dropped,
                    "inbound event ring saturated; dropping oldest"
                );
                ring.last_warn = Some(Instant::now());
            }
        }
        ring.buf.push_back(payload);
        true
    }

    /// Drains every buffered payload for the ring keyed by `type_id`, oldest
    /// first. Returns an empty `Vec` when the type was never declared. The ring
    /// lock is released before the caller deserializes, so a slow `from_value`
    /// never blocks the reader thread's ingest.
    pub(crate) fn drain_type(&self, type_id: TypeId) -> Vec<Value> {
        let Some(ring) = self.by_type.get(&type_id) else {
            return Vec::new();
        };
        let mut ring = ring.lock().unwrap_or_else(|e| e.into_inner());
        Vec::from(std::mem::take(&mut ring.buf))
    }

    fn from_decls_with_cap(decls: &[EventDecl], cap: usize) -> Self {
        let mut by_name = HashMap::new();
        let mut by_type = HashMap::new();
        for decl in decls {
            let ring: Ring = Arc::new(Mutex::new(RingBuf::default()));
            by_name.insert(decl.name.clone(), ring.clone());
            by_type.insert(decl.type_id, ring);
        }
        Self { by_name, by_type, cap }
    }
}
```

Note: `from_decls_with_cap` is `fn` (private) — used by `from_decls` and the tests in the same module, so it needs no visibility modifier.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p ratatui-ozma events::`
Expected: PASS (5 tests).

- [ ] **Step 6: Lint + commit**

```bash
cargo fmt
cargo clippy -p ratatui-ozma --all-targets
git add sdk/ratatui-ozma/src/events.rs sdk/ratatui-ozma/src/lib.rs
git commit -m "feat(ratatui-ozma): add EventQueues bounded-ring inbound event buffer"
```

---

### Task 2: `IncomingEvent` wire type

**Files:**
- Modify: `sdk/ratatui-ozma/src/protocol.rs`
- Test: inline `#[cfg(test)] mod tests` in `protocol.rs`

**Interfaces:**
- Produces: `pub(crate) struct IncomingEvent { handle: String, event: String, payload: Value }` (fields `pub(crate)`), deserialized from `{"op":"event","handle":…,"event":…,"payload":…}`.

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block in `sdk/ratatui-ozma/src/protocol.rs`:

```rust
    #[test]
    fn incoming_event_deserializes() {
        let e: IncomingEvent = serde_json::from_str(
            r#"{"op":"event","handle":"h","event":"hello","payload":{"message":"hi"}}"#,
        )
        .unwrap();
        assert_eq!(e.handle, "h");
        assert_eq!(e.event, "hello");
        assert_eq!(e.payload, serde_json::json!({"message":"hi"}));
    }

    #[test]
    fn incoming_event_without_payload_is_null() {
        let e: IncomingEvent =
            serde_json::from_str(r#"{"op":"event","handle":"h","event":"ping"}"#).unwrap();
        assert_eq!(e.payload, Value::Null);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ratatui-ozma protocol::tests::incoming_event`
Expected: FAIL to compile — `IncomingEvent` not found.

- [ ] **Step 3: Write the implementation**

Add after the `IncomingCall` struct in `sdk/ratatui-ozma/src/protocol.rs` (the `Value` and `Deserialize` imports are already present at the top):

```rust
/// An inbound one-way `event` frame forwarded from a page's `window.ozma.emit`.
#[derive(Debug, Deserialize)]
pub(crate) struct IncomingEvent {
    /// The view handle the event targets.
    pub(crate) handle: String,
    /// The declared event name (`add_event::<T>(name)`).
    pub(crate) event: String,
    /// The single payload value (any JSON shape; absent deserializes as null).
    #[serde(default)]
    pub(crate) payload: Value,
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ratatui-ozma protocol::tests::incoming_event`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add sdk/ratatui-ozma/src/protocol.rs
git commit -m "feat(ratatui-ozma): add IncomingEvent wire type for one-way page events"
```

---

### Task 3: `Webview::add_event` builder

**Files:**
- Modify: `sdk/ratatui-ozma/src/webview.rs`
- Test: inline `#[cfg(test)] mod tests` in `webview.rs`

**Interfaces:**
- Consumes: `EventDecl` (Task 1).
- Produces: `pub fn add_event<T: DeserializeOwned + 'static>(self, name: impl Into<String>) -> Self` on `Webview`; a private `event_decls: Vec<EventDecl>` field on `Webview` (read by `Ozma::register` in Task 4).

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block in `sdk/ratatui-ozma/src/webview.rs`:

```rust
    #[test]
    fn add_event_records_decl() {
        struct Hello;
        let wv = Webview::inline("x").add_event::<Hello>("hello");
        assert_eq!(wv.event_decls.len(), 1);
        assert_eq!(wv.event_decls[0].name, "hello");
        assert_eq!(wv.event_decls[0].type_id, std::any::TypeId::of::<Hello>());
    }

    #[test]
    fn add_event_enables_bridge_for_url() {
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
        struct A;
        struct B;
        let _ = Webview::inline("x")
            .add_event::<A>("dup")
            .add_event::<B>("dup");
    }

    #[test]
    #[should_panic(expected = "already registered")]
    fn add_event_rejects_duplicate_type() {
        struct A;
        let _ = Webview::inline("x")
            .add_event::<A>("one")
            .add_event::<A>("two");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ratatui-ozma webview::tests::add_event`
Expected: FAIL to compile — `add_event` / `event_decls` not found.

- [ ] **Step 3: Add the field and import**

In `sdk/ratatui-ozma/src/webview.rs`, add to the top `use` block:

```rust
use crate::events::EventDecl;
use std::any::TypeId;
```

Add the field to the `Webview` struct and initialize it in all three constructors (`inline`, `url`, `dir`):

```rust
pub struct Webview {
    pub(crate) kind: RegisterKind,
    pub(crate) handlers: HashMap<String, BoxedHandler>,
    pub(crate) event_decls: Vec<EventDecl>,
}
```

Each constructor currently ends `handlers: HashMap::new(),` inside its `Self { … }`; add `event_decls: Vec::new(),` alongside it in `inline`, `url`, and `dir`.

- [ ] **Step 4: Write the `add_event` method**

Add this method to `impl Webview`, placed after `on` (keep `pub` methods grouped):

```rust
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
        if let RegisterKind::Url { bridge, .. } = &mut self.kind {
            *bridge = true;
        }
        self
    }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p ratatui-ozma webview::tests::add_event`
Expected: PASS (4 tests). The existing `webview::tests` continue to pass.

- [ ] **Step 6: Lint + commit**

```bash
cargo fmt
cargo clippy -p ratatui-ozma --all-targets
git add sdk/ratatui-ozma/src/webview.rs
git commit -m "feat(ratatui-ozma): add Webview::add_event builder for inbound events"
```

---

### Task 4: `read_events` + reader-thread ingest wiring

**Files:**
- Modify: `sdk/ratatui-ozma/src/webview.rs` (`WebviewHandle` gains `events`, `read_events`, updated `new_shared`)
- Modify: `sdk/ratatui-ozma/src/session.rs` (`EventRegistry` threaded through `connect`/`spawn_reader`; `PendingRegister.events`; `register` builds + installs `EventQueues`; reader `op=="event"` branch)
- Test: inline tests in `webview.rs` and `session.rs`

**Interfaces:**
- Consumes: `EventQueues`, `EventRegistry` (Task 1); `IncomingEvent` (Task 2); `Webview.event_decls` (Task 3).
- Produces: `pub fn read_events<T: DeserializeOwned + 'static>(&self) -> Vec<T>` on `WebviewHandle`; `WebviewHandle::new_shared(id, events, writer)` signature gains an `Arc<EventQueues>` second parameter.

- [ ] **Step 1: Write the failing `read_events` test (webview.rs)**

Add to `#[cfg(test)] mod tests` in `sdk/ratatui-ozma/src/webview.rs`:

```rust
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
        events.ingest("hello", json!({"nope": 1})); // fails to deserialize -> skipped
        events.ingest("hello", json!({"message": "b"}));

        let (sock, _b) = std::os::unix::net::UnixStream::pair().unwrap();
        let writer: SharedWriter = Arc::new(Mutex::new(sock));
        let handle = WebviewHandle::new_shared(
            Arc::new(Mutex::new("h".to_owned())),
            events,
            writer,
        );

        let got = handle.read_events::<Hello>();
        assert_eq!(
            got,
            vec![
                Hello { message: "a".into() },
                Hello { message: "b".into() }
            ]
        );
        // Drained: a second read is empty.
        assert!(handle.read_events::<Hello>().is_empty());
    }
```

The existing `id_reflects_slot_update` test calls `WebviewHandle::new_shared(slot.clone(), writer)` — update it to pass an empty queues arg:

```rust
        let handle = WebviewHandle::new_shared(
            slot.clone(),
            Arc::new(crate::events::EventQueues::default()),
            writer,
        );
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p ratatui-ozma webview::tests::read_events`
Expected: FAIL to compile — `read_events` / new `new_shared` signature not present.

- [ ] **Step 3: Implement the `WebviewHandle` changes (webview.rs)**

Add to the top `use` block:

```rust
use crate::events::EventQueues;
```

Add the field to the struct:

```rust
pub struct WebviewHandle {
    id: Arc<Mutex<String>>,
    events: Arc<EventQueues>,
    writer: SharedWriter,
}
```

Add `read_events` to `impl WebviewHandle` after `emit` (keep `pub` methods first):

```rust
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
```

Add `use std::any::TypeId;` to the top `use` block (if not already present from another edit).

Update `new_shared` (still `pub(crate)`, declared after `pub` methods) to take and store `events`, with mutable/structural params unchanged (all by-value, none mutable):

```rust
    pub(crate) fn new_shared(
        id: Arc<Mutex<String>>,
        events: Arc<EventQueues>,
        writer: SharedWriter,
    ) -> Self {
        Self { id, events, writer }
    }
```

- [ ] **Step 4: Run the webview test to verify it passes**

Run: `cargo test -p ratatui-ozma webview::tests::read_events`
Expected: PASS. (`session.rs` will not compile yet — that is fixed in the next steps; run the targeted webview test which only needs `webview.rs`.)

- [ ] **Step 5: Write the failing reader-routing test (session.rs)**

Add to `#[cfg(test)] mod tests` in `sdk/ratatui-ozma/src/session.rs` (mirrors `reader_thread_inserts_compositing_into_shared_map`):

```rust
    #[test]
    fn reader_thread_routes_event_into_registered_queues() {
        use crate::events::{EventDecl, EventQueues, EventRegistry};
        use std::os::unix::net::UnixListener;

        struct Hello;
        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("ev.sock");
        let listener = UnixListener::bind(&sock_path).unwrap();

        let decls = vec![EventDecl {
            name: "hello".into(),
            type_id: std::any::TypeId::of::<Hello>(),
        }];
        let queues = Arc::new(EventQueues::from_decls(&decls));
        let events: EventRegistry = Arc::new(Mutex::new(HashMap::new()));
        events.lock().unwrap().insert("h1".to_owned(), queues.clone());

        let handlers: HandlerRegistry = Arc::new(Mutex::new(HashMap::new()));
        let pending: PendingRegisters = Arc::new(Mutex::new(VecDeque::new()));
        let pending_compositing: PendingCompositing = Arc::new(Mutex::new(HashMap::new()));

        let client = UnixStream::connect(&sock_path).unwrap();
        let writer: SharedWriter = Arc::new(Mutex::new(client.try_clone().unwrap()));
        let (server_conn, _) = listener.accept().unwrap();

        spawn_reader(
            client,
            writer.clone(),
            handlers,
            pending,
            pending_compositing,
            events.clone(),
            Arc::new(AtomicBool::new(false)),
        );

        let mut server = server_conn;
        writeln!(
            server,
            r#"{{"op":"event","handle":"h1","event":"hello","payload":{{"n":7}}}}"#
        )
        .unwrap();
        server.flush().unwrap();

        std::thread::sleep(std::time::Duration::from_millis(50));

        assert_eq!(queues.drain_type(std::any::TypeId::of::<Hello>()), vec![serde_json::json!({"n":7})]);
    }
```

- [ ] **Step 6: Run to verify it fails**

Run: `cargo test -p ratatui-ozma session::tests::reader_thread_routes_event`
Expected: FAIL to compile — `spawn_reader` has no `events` parameter; the `op=="event"` branch does not exist.

- [ ] **Step 7: Thread `EventRegistry` and add the reader branch (session.rs)**

The registry is owned by `Ozma` and installed at `register()` time (direct install on the registering thread). The reader thread receives a clone purely to **route** inbound events; reconnect re-install is added in Task 5. This keeps Task 4 fully compiling on its own and leaves reconnect as a clean red→green for Task 5.

Add to the top `use` block:

```rust
use crate::events::{EventQueues, EventRegistry};
```

and add `IncomingEvent` to the existing `use crate::protocol::{…}` line (do not add a second `crate::protocol::` line).

**7a.** Add an `events` field to `Registration` (NOT `PendingRegister`):

```rust
struct Registration {
    kind: RegisterKind,
    handle_slot: Arc<Mutex<String>>,
    handlers: Arc<HashMap<String, BoxedHandler>>,
    events: Arc<EventQueues>,
}
```

**7b.** Add an `events: EventRegistry` field to the `Ozma` struct, and in `connect()` create the registry next to `handlers`/`registrations`:

```rust
        let events: EventRegistry = Arc::new(Mutex::new(HashMap::new()));
```

Pass `events.clone()` into the initial `spawn_reader(...)` call (new 6th arg, before `disconnected.clone()`); capture a clone for the reconnect thread closure (`let events2 = events.clone();`) and pass `&events2` into `attempt_reconnect(...)`; and set `events` in the returned `Ozma { … }`.

**7c.** In `register()`, destructure `event_decls`, build the queues, and install them under the minted handle directly:

```rust
        let Webview { kind, handlers, event_decls } = webview;
        let handlers = Arc::new(handlers);
        let events = Arc::new(EventQueues::from_decls(&event_decls));
```

After `let handle = rx.recv().map_err(|_| OzmaError::Disconnected)??;`, install the queues, then store/return them:

```rust
        if let Ok(mut map) = self.events.lock() {
            map.insert(handle.clone(), events.clone());
        }
        let handle_slot = Arc::new(Mutex::new(handle));
```

Add `events: events.clone()` to the `Registration { … }` it stores, and pass `events` as the new second arg to `WebviewHandle::new_shared(handle_slot, events, self.writer.clone())`.

NOTE: a view cannot be mounted (and so cannot `emit`) until after `register()` returns and the app draws the mount OSC, so no inbound event can arrive before this install — direct install has no lost-event window in practice.

**7d.** Change `spawn_reader`'s signature to accept the registry for routing (all params are owned by-value shares, none `mut`; insert `events` before `disconnected` to match the `connect`/`attempt_reconnect` call sites):

```rust
fn spawn_reader(
    stream: UnixStream,
    writer: SharedWriter,
    handlers: HandlerRegistry,
    pending: PendingRegisters,
    pending_compositing: PendingCompositing,
    events: EventRegistry,
    disconnected: Arc<AtomicBool>,
) {
```

**7e.** Add the `op=="event"` branch to the op ladder, after the `compositing` branch and before the `else if let Ok(reply)` register-reply branch:

```rust
            } else if op == "event" {
                if let Ok(ev) = serde_json::from_str::<IncomingEvent>(trimmed) {
                    let queues = events
                        .lock()
                        .ok()
                        .and_then(|map| map.get(&ev.handle).cloned());
                    if let Some(queues) = queues
                        && !queues.ingest(&ev.event, ev.payload)
                    {
                        tracing::debug!(
                            handle = ev.handle,
                            event = ev.event,
                            "inbound event for an undeclared name dropped"
                        );
                    }
                }
```

**7f.** Add the `events` parameter to `attempt_reconnect` so its `spawn_reader(...)` call compiles (the install/replay body is added in Task 5). Place `events: &EventRegistry` after `registrations`, before `token`:

```rust
fn attempt_reconnect(
    writer: &SharedWriter,
    handlers: &HandlerRegistry,
    pending: &PendingRegisters,
    pending_compositing: &PendingCompositing,
    disconnected: &Arc<AtomicBool>,
    generation: &Arc<AtomicU64>,
    registrations: &Arc<Mutex<Vec<Registration>>>,
    events: &EventRegistry,
    token: &str,
) {
```

Pass `events.clone()` into the `spawn_reader(...)` call inside `attempt_reconnect` (new 6th arg, before `disconnected.clone()`).

- [ ] **Step 8: Run the full SDK test suite to verify everything passes**

Run: `cargo test -p ratatui-ozma`
Expected: PASS — the new `reader_thread_routes_event_into_registered_queues`, `read_events_drains_…`, the updated `id_reflects_slot_update`, and all pre-existing tests. The crate compiles fully (initial-connect routing works); reconnect re-install is completed in Task 5.

- [ ] **Step 9: Lint + commit**

```bash
cargo fmt
cargo clippy -p ratatui-ozma --all-targets
git add sdk/ratatui-ozma/src/webview.rs sdk/ratatui-ozma/src/session.rs
git commit -m "feat(ratatui-ozma): route inbound events to per-handle queues and add read_events"
```

---

### Task 5: Reconnect replay of event queues

**Files:**
- Modify: `sdk/ratatui-ozma/src/session.rs` (`attempt_reconnect` threads `EventRegistry`; replays `events`; removes the old handle key)
- Test: `sdk/ratatui-ozma/tests/integration.rs` (reconnect preserves a working `read_events`)

**Interfaces:**
- Consumes: `EventRegistry`, `Registration.events`, and the `events` parameter already threaded into `attempt_reconnect` (Task 4).
- Produces: no new public surface; completes `attempt_reconnect`'s re-install of per-handle queues under the new handle.

- [ ] **Step 1: Write the failing integration test**

Add to `sdk/ratatui-ozma/tests/integration.rs`:

```rust
#[test]
fn reconnect_preserves_inbound_events() {
    use std::time::Duration;
    #[derive(serde::Deserialize, PartialEq, Debug)]
    struct Hello {
        message: String,
    }

    let pair = support::ReconnectPair::start("view-ev1", "view-ev2");
    with_env(&pair.first.sock_path.clone(), || {
        let ozma = Ozma::connect().unwrap();
        let handle = ozma
            .register(Webview::inline("x").add_event::<Hello>("hello"))
            .unwrap();

        let term_bytes = SharedBuf(Arc::new(Mutex::new(Vec::new())));
        let mut backend = OzmaBackend::new(CrosstermBackend::new(term_bytes.clone()), &ozma);
        Backend::draw(&mut backend, std::iter::empty::<(u16, u16, &Cell)>()).unwrap();

        drop(pair.first);
        std::thread::sleep(Duration::from_millis(200));
        // NOTE: ENV_LOCK is held by with_env, serializing env var access.
        unsafe { std::env::set_var("OZMA_SOCK", &pair.second.sock_path) };
        Backend::draw(&mut backend, std::iter::empty::<(u16, u16, &Cell)>()).unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while handle.id() == "view-ev1" {
            assert!(std::time::Instant::now() < deadline, "reconnect did not complete");
            std::thread::sleep(Duration::from_millis(50));
        }

        // The page emits to the NEW handle after reconnect; read_events must see it.
        pair.second.send(json!({
            "op": "event", "handle": "view-ev2", "event": "hello", "payload": { "message": "post" }
        }));

        let got = loop {
            let evs = handle.read_events::<Hello>();
            if !evs.is_empty() {
                break evs;
            }
            assert!(std::time::Instant::now() < deadline, "event never arrived after reconnect");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(got, vec![Hello { message: "post".into() }]);
    });
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p ratatui-ozma --test integration reconnect_preserves_inbound_events`
Expected: FAIL — Task 4 threaded the `events` param into `attempt_reconnect` (so it compiles) but does not yet re-install the per-handle queues under the new handle, so the post-reconnect event finds no queues for `view-ev2` and is dropped; the read loop times out.

- [ ] **Step 3: Re-install the queues under the new handle in `attempt_reconnect`**

After the existing handler-map swap inside the re-registration loop (`map.remove(&old); map.insert(new_handle.clone(), reg.handlers.clone());`), add the matching event-registry swap. Re-installing under the new handle is what makes post-reconnect events reach `read_events`; removing the old key prevents a stale-handle leak across repeated reconnects:

```rust
        if let Ok(mut map) = events.lock() {
            map.remove(&old);
            map.insert(new_handle.clone(), reg.events.clone());
        }
```

(`old` is the prior handle already read from `reg.handle_slot` for the handler swap; `reg.events` is the same `Arc<EventQueues>` the `WebviewHandle` holds, so buffered events survive the reconnect.)

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p ratatui-ozma --test integration reconnect_preserves_inbound_events`
Expected: PASS.

- [ ] **Step 5: Full SDK suite + lint + commit**

```bash
cargo test -p ratatui-ozma
cargo fmt
cargo clippy -p ratatui-ozma --all-targets
git add sdk/ratatui-ozma/src/session.rs sdk/ratatui-ozma/tests/integration.rs
git commit -m "feat(ratatui-ozma): replay inbound event queues across reconnect"
```

---

### Task 6: Page bridge `emit` + preload assertion

**Files:**
- Modify: `src/webview/render/ozma_bridge.js`
- Modify: `src/webview/render/preload.rs`
- Test: `src/webview/render/preload.rs` (existing `dynamic_preload_injects_only_the_ozma_bridge`)

**Interfaces:**
- Produces: a `window.ozma.emit(event, payload)` page method emitting `{ kind: 'ozma.emit', event, payload }` via `cef.emit`.

- [ ] **Step 1: Write the failing assertion**

In `src/webview/render/preload.rs`, add to `dynamic_preload_injects_only_the_ozma_bridge` (after the existing `kind: 'ozma.call'` assert):

```rust
        assert!(OZMA_BRIDGE_JS.contains("kind: 'ozma.emit'"));
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p ozmux-gui webview::render::preload::tests::dynamic_preload_injects_only_the_ozma_bridge`
Expected: FAIL — the bridge does not yet contain `kind: 'ozma.emit'`.

- [ ] **Step 3: Add `emit` to the bridge**

In `src/webview/render/ozma_bridge.js`, add an `emit` method to the `api` object, immediately after the `off:` function (before the closing `};` of `var api = { … }`):

```js
    emit: function (event, payload) {
      cef.emit({ kind: 'ozma.emit', event: event, payload: encodeParam(payload) });
    },
```

(`encodeParam` is already defined above and tags a top-level `Uint8Array`; `emit` sends no `reqId` and creates no `calls` entry, so it cannot leak.)

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p ozmux-gui webview::render::preload::tests`
Expected: PASS (all three preload tests).

- [ ] **Step 5: Lint + commit**

```bash
make fix-lint
git add src/webview/render/ozma_bridge.js src/webview/render/preload.rs
git commit -m "feat(webview): add window.ozma.emit one-way page->host bridge method"
```

---

### Task 7: Host `on_ozmux_emit_frame` forwarder

**Files:**
- Modify: `src/webview/render.rs`
- Test: inline `#[cfg(test)] mod tests` in `src/webview/render.rs`

**Interfaces:**
- Consumes: `OzmuxFrame`, `WebviewOwner`, `ConnectionWriters` (existing).
- Produces: the observer `on_ozmux_emit_frame`, registered in `RenderPlugin::build`, that forwards `kind:"ozma.emit"` frames as `{"op":"event",…}` to the owning connection.

- [ ] **Step 1: Write the failing test**

Add to `#[cfg(test)] mod tests` in `src/webview/render.rs` (mirrors `ozmux_call_frame_pushes_call_to_owner_connection`):

```rust
    #[test]
    fn ozmux_emit_frame_pushes_event_to_owner_connection() {
        use crossbeam_channel::unbounded;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        let writers = ConnectionWriters::default();
        let (tx, rx) = unbounded::<String>();
        writers.insert(7, tx);
        app.insert_resource(writers);
        app.add_observer(on_ozmux_emit_frame);

        let webview = app
            .world_mut()
            .spawn(WebviewOwner {
                connection_id: 7,
                handle: "H".into(),
            })
            .id();

        app.world_mut().trigger(Receive {
            webview,
            payload: OzmuxFrame(serde_json::json!({
                "kind": "ozma.emit", "event": "hello", "payload": {"message": "hi"}
            })),
        });

        let line = rx.try_recv().expect("an event was pushed");
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["op"], "event");
        assert_eq!(v["handle"], "H");
        assert_eq!(v["event"], "hello");
        assert_eq!(v["payload"]["message"], "hi");
    }

    #[test]
    fn ozmux_emit_frame_without_owner_is_dropped() {
        use crossbeam_channel::unbounded;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        let writers = ConnectionWriters::default();
        let (tx, rx) = unbounded::<String>();
        writers.insert(7, tx);
        app.insert_resource(writers);
        app.add_observer(on_ozmux_emit_frame);

        // A webview entity with no WebviewOwner component.
        let webview = app.world_mut().spawn_empty().id();
        app.world_mut().trigger(Receive {
            webview,
            payload: OzmuxFrame(serde_json::json!({
                "kind": "ozma.emit", "event": "hello", "payload": null
            })),
        });

        assert!(rx.try_recv().is_err(), "no owner ⇒ nothing forwarded");
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p ozmux-gui webview::render::tests::ozmux_emit_frame`
Expected: FAIL to compile — `on_ozmux_emit_frame` not found.

- [ ] **Step 3: Add the kind constant and observer**

In `src/webview/render.rs`, add the constant next to `OZMA_CALL_KIND`:

```rust
/// The `kind` discriminator routing a `Receive<OzmuxFrame>` to the one-way
/// inbound-event forwarder (`on_ozmux_emit_frame`). Emitted by `ozma_bridge.js`.
const OZMA_EMIT_KIND: &str = "ozma.emit";
```

Add the observer (place it after `on_ozmux_call_frame` and its `reject_ozmux_call` helper, before the load loggers):

```rust
/// Inbound (one-way): a `window.ozma.emit` arrives as a `Receive<OzmuxFrame>`
/// with `kind:"ozma.emit"`. The trusted caller is `frame.webview` (bound per
/// webview by `bevy_cef`); its `WebviewOwner` names the registering connection.
/// The event is forwarded as a fire-and-forget `{op:"event"}` line — no reqId,
/// no reply, no `OzmuxRpc` tracking. A missing owner or unavailable connection
/// drops the event (debug-logged); there is no page Promise to settle.
///
/// Registered on the shared `Receive<OzmuxFrame>` event (not a second
/// `JsEmitEventPlugin`); non-`ozma.emit` frames return early on `OZMA_EMIT_KIND`.
fn on_ozmux_emit_frame(
    frame: On<Receive<OzmuxFrame>>,
    writers: Res<ConnectionWriters>,
    owners: Query<&WebviewOwner>,
) {
    let payload = &frame.payload.0;
    if payload.get("kind").and_then(Value::as_str) != Some(OZMA_EMIT_KIND) {
        return;
    }
    let event = payload
        .get("event")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let body = payload.get("payload").cloned().unwrap_or(Value::Null);

    let Ok(owner) = owners.get(frame.webview) else {
        tracing::debug!("ozma.emit frame for a webview with no owner; dropping");
        return;
    };
    let line = serde_json::json!({
        "op": "event", "handle": owner.handle, "event": event, "payload": body
    })
    .to_string();
    if !writers.send(owner.connection_id, line) {
        tracing::debug!(handle = owner.handle, "ozma.emit owner connection unavailable; dropping");
    }
}
```

Register it in `RenderPlugin::build`, extending the existing observer chain:

```rust
        app.add_plugins(JsEmitEventPlugin::<OzmuxFrame>::default())
            .add_observer(on_ozmux_call_frame)
            .add_observer(on_ozmux_emit_frame)
            .add_observer(on_webview_address_changed)
            .add_observer(drop_ozmux_inflight_on_webview_despawn)
            .add_observer(log_webview_load_started)
            .add_observer(log_webview_load_finished)
            .add_observer(log_webview_load_error)
            .add_systems(Update, sync_focused_webview.after(OzmuxSystems::Input));
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p ozmux-gui webview::render::tests::ozmux_emit_frame`
Expected: PASS (2 tests).

- [ ] **Step 5: Lint + commit**

```bash
cargo fmt
cargo clippy -p ozmux-gui --all-targets
git add src/webview/render.rs
git commit -m "feat(webview): forward one-way ozma.emit page events to the owning connection"
```

---

### Task 8: `@ozma/web` `emit` client

**Files:**
- Modify: `sdk/ozma-web/src/ozma.ts`
- Modify: `sdk/ozma-web/src/ozma.test.ts`

**Interfaces:**
- Produces: `OzmaApi.emit(event: string, payload?: unknown): void` and the `ozma.emit` delegate.

- [ ] **Step 1: Write the failing tests**

In `sdk/ozma-web/src/ozma.test.ts`, extend the delegation test's mock and assertions, and add `emit` to the throw test:

In `delegates call/on/off to the injected bridge`, add an `emit` mock and assertion:

```ts
    const emit = vi.fn();
    g.ozma = { call, on, off, emit } as unknown as OzmaApi;
```
```ts
    ozma.emit('hello', { message: 'hi' });
```
```ts
    expect(emit).toHaveBeenCalledWith('hello', { message: 'hi' });
```

In `throws a descriptive error when the bridge is absent`, add:

```ts
    expect(() => ozma.emit('hello', {})).toThrow(/window\.ozma is unavailable/);
```

- [ ] **Step 2: Run to verify it fails**

Run: `pnpm --filter @ozma/web test`
Expected: FAIL — `ozma.emit` is not a function / type error.

- [ ] **Step 3: Add `emit` to the interface and the const**

In `sdk/ozma-web/src/ozma.ts`, add to the `OzmaApi` interface (after `off`):

```ts
  /** Sends a one-way event to the host app (fire-and-forget; no reply). */
  emit(event: string, payload?: unknown): void;
```

And to the `ozma` const object (after `off`):

```ts
  emit(event: string, payload?: unknown): void {
    resolve().emit(event, payload);
  },
```

- [ ] **Step 4: Run to verify it passes**

Run: `pnpm --filter @ozma/web test`
Expected: PASS.

- [ ] **Step 5: Typecheck, lint, commit**

```bash
pnpm check-types
pnpm lint:fix
git add sdk/ozma-web/src/ozma.ts sdk/ozma-web/src/ozma.test.ts
git commit -m "feat(ozma-web): add ozma.emit one-way event client method"
```

---

### Task 9: End-to-end integration test

**Files:**
- Modify: `sdk/ratatui-ozma/tests/integration.rs`

**Interfaces:**
- Consumes: the full SDK inbound-event path (Tasks 1–5).

- [ ] **Step 1: Write the test**

Add to `sdk/ratatui-ozma/tests/integration.rs` (peer of `emit_reaches_the_server`):

```rust
#[test]
fn inbound_event_is_buffered_and_read() {
    use std::time::{Duration, Instant};
    #[derive(serde::Deserialize, PartialEq, Debug)]
    struct Hello {
        message: String,
    }

    let server = FakeServer::start("view-ev");
    with_env(&server.sock_path.clone(), || {
        let ozma = Ozma::connect().unwrap();
        let handle = ozma
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
        assert_eq!(events, vec![Hello { message: "hi".into() }]);
    });
}
```

- [ ] **Step 2: Run to verify it passes**

Run: `cargo test -p ratatui-ozma --test integration inbound_event_is_buffered_and_read`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add sdk/ratatui-ozma/tests/integration.rs
git commit -m "test(ratatui-ozma): end-to-end inbound event buffered and read"
```

---

### Task 10: Round-trip example

**Files:**
- Modify: `sdk/ratatui-ozma/examples/ratatui_webview.rs`

**Interfaces:**
- Consumes: `Webview::add_event`, `WebviewHandle::read_events` (Tasks 3–4).

- [ ] **Step 1: Add an inbound event type and declare it**

In `sdk/ratatui-ozma/examples/ratatui_webview.rs`, add near the top of `main` an event type:

```rust
    #[derive(serde::Deserialize)]
    struct HelloMsg {
        message: String,
    }
```

Update the inline HTML's `<script>` so the input emits on Enter (replace the existing `<script>…</script>` block):

```rust
        "<script>",
        "window.ozma.call('ping','hi').then(v=>out.textContent='ping → '+v);",
        "window.ozma.on('tick',n=>tick.textContent='tick #'+n);",
        "var inp=document.getElementById('in');",
        "inp.addEventListener('keydown',function(e){",
        "  if(e.key==='Enter'){ window.ozma.emit('hello',{message:inp.value}); inp.value=''; }",
        "});",
        "inp.focus();",
        "</script></body>"
```

Register the event on the builder (add `.add_event::<HelloMsg>("hello")` to the `Webview::inline(html)` chain, after `.on("ping", …)`):

```rust
            .on("ping", |arg: String| {
                Ok::<_, RpcError>(format!("pong:{arg}"))
            })
            .add_event::<HelloMsg>("hello"),
```

- [ ] **Step 2: Drain the events in the loop**

Pass the new type into `run` via the loop. In `run`, add a `last_msg` string state and drain each frame. Add after the existing tick block inside `loop`:

```rust
        for HelloMsg { message } in view.read_events::<HelloMsg>() {
            last_msg = message;
        }
```

Declare `let mut last_msg = String::new();` near `let mut web_focused = false;`, and render it — change the status `Paragraph` row to include the last message:

```rust
            f.render_widget(
                Paragraph::new(format!(
                    "Alt+l focus webview · Alt+h leave · q quit · last: {last_msg}"
                )),
                rows[0],
            );
```

Because `HelloMsg` is declared inside `main`, move its `#[derive(serde::Deserialize)] struct HelloMsg { … }` to module scope (above `fn main`) so `run` can name it, and reference it as `HelloMsg` in both places.

- [ ] **Step 3: Verify it builds**

Run: `cargo build -p ratatui-ozma --example ratatui_webview`
Expected: builds with no errors or warnings.

- [ ] **Step 4: Commit**

```bash
cargo fmt
git add sdk/ratatui-ozma/examples/ratatui_webview.rs
git commit -m "docs(ratatui-ozma): demonstrate window.ozma.emit round trip in the example"
```

---

## Final verification

After all tasks:

- [ ] `cargo test -p ratatui-ozma` — all SDK unit + integration tests pass.
- [ ] `cargo test -p ozmux-gui webview::render` — host forwarder + preload tests pass (requires `make setup-cef`).
- [ ] `pnpm --filter @ozma/web test && pnpm check-types && pnpm lint` — TS green.
- [ ] `cargo clippy --workspace --all-targets` clean; `cargo fmt --check` clean.
- [ ] Manual E2E (optional): `cargo run --features debug`, then in a pane run the example and type into the input + Enter; confirm the `last:` status updates.
