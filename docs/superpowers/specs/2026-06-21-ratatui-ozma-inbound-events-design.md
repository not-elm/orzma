# ratatui-ozma inbound events (Webview → app, one-way)

Status: design (2026-06-21). Produced via the brainstorming flow; pending a
spec review.

## 1. Goal & problem

`ratatui-ozma` today has three of the four page/app channels:

| Direction | Mechanism | Shape |
| --- | --- | --- |
| app → page | `WebviewHandle::emit(event, payload)` → page `window.ozma.on` | one-way, fire-and-forget |
| page → app | `window.ozma.call(method, params)` → `Webview::on(method, closure)` | request/response RPC |
| app ↔ host | OSC mount + control-socket focus/navigate | — |

The missing quadrant is **page → app, one-way, poll-based**: a webview wants to
*notify* the host program of something (a button press, a form value, a
selection) without the host having to register a reply-producing closure that
runs on the SDK reader thread and shares state with the UI through an
`Arc<Mutex<…>>`. The memo sketch (`docs/memo.md`) asks for:

```rust
#[derive(Deserialize)]
struct HelloEvent { message: String }

let view = Webview::url("…").add_event::<HelloEvent>();
loop {
    let events = view.read_events::<HelloEvent>();
}
```

i.e. the app declares the events it accepts, then **drains a typed queue in its
own render loop** — the same ergonomic shape ratatui apps already use for
`crossterm` events.

This is purely additive. It coexists with `.on` (RPC) and `emit` (app→page);
nothing about those changes.

## 2. Design decisions (locked)

| # | Decision | Choice |
| --- | --- | --- |
| 1 | Transport | **Full-stack one-way `emit`**: a genuine fire-and-forget primitive across all four layers (page SDK, bridge JS, host forwarder, SDK drain). No reqId, no reply, no RPC tracking. |
| 2 | Name binding | **Explicit string** at `add_event::<T>("name")`; `read_events::<T>()` keyed by `TypeId`. Invariant: **1:1 type ↔ name**. |
| 3 | Read API | `read_events::<T>() -> Vec<T>`; a payload that fails to deserialize into `T` is **dropped + `tracing::warn!`**, never surfaced to the caller. |
| 4 | Buffering | **Bounded ring per type** (default cap 1024). Overflow drops the **oldest** event + a throttled `tracing::warn!`. |

## 3. End-to-end data path

A page emit travels four layers; it mirrors the existing `call` path but strips
reqId/reply/`OzmuxRpc`:

```
window.ozma.emit("hello", { message })                      // @ozma/web  (NEW)
  └─ cef.emit({ kind:'ozma.emit', event, payload })          // bridge JS  (NEW kind)
       └─ host observer on Receive<OzmuxFrame>, kind=="ozma.emit"   // src/webview/render.rs (NEW)
            owner = WebviewOwner(frame.webview)              // trusted, bevy_cef-bound (never page-supplied)
            writers.send(owner.connection_id,
                {"op":"event","handle":owner.handle,"event":…,"payload":…})   // NEW wire line
                 └─ SDK reader thread: op=="event"           // session.rs (NEW branch)
                      route payload into the handle's named ring buffer (bounded)
                           └─ view.read_events::<T>()        // WebviewHandle (NEW)
```

Trust model is unchanged from `call`: the caller identity is `frame.webview`
(bound per-webview by `bevy_cef`, never the JS payload); its `WebviewOwner`
names the registering connection; only that connection receives the forwarded
line. The event name and payload are untrusted page data — the app decides what
to do with them, and only **declared** names are buffered (unknown names
dropped).

## 4. SDK public API (`ratatui-ozma`)

```rust
impl Webview {
    /// Declares an inbound event the page may send, binding wire name → type `T`.
    /// Enables the `window.ozma` bridge for `url` webviews (like `on`); a no-op
    /// for `inline`/`dir`, which are always bridged.
    ///
    /// # Panics
    /// Panics if `name` (or the type `T`) is already registered on this builder
    /// — the type ↔ name mapping must be 1:1.
    pub fn add_event<T: DeserializeOwned + 'static>(self, name: impl Into<String>) -> Self;
}

impl WebviewHandle {
    /// Drains and returns every buffered event of type `T`, oldest first.
    /// Payloads that fail to deserialize into `T` are dropped and logged.
    /// Returns an empty `Vec` if `T` was never declared via `add_event`.
    pub fn read_events<T: DeserializeOwned + 'static>(&self) -> Vec<T>;
}
```

Concrete usage:

```rust
#[derive(Deserialize)]
struct HelloEvent { message: String }

let view = ozma.register(
    Webview::url("https://app.example")
        .add_event::<HelloEvent>("hello"),
)?;

loop {
    terminal.draw(/* … */)?;
    for e in view.read_events::<HelloEvent>() {
        // handle each event in the app's own loop, alongside crossterm events
    }
}
```

Page side:

```js
window.ozma.emit("hello", { message: "hi" });
```

Rules:

- **1:1 type ↔ name.** Duplicate type *or* duplicate name on the same builder
  **panics at builder time**. This reuses the *panic mechanism* of
  `Webview::on`'s reserved-namespace `assert!`, but the duplicate check is a
  **new invariant** `add_event` enforces — `on` itself silently overwrites a
  duplicate method in its `HashMap`. (Want two events with the same shape? Use
  two newtypes.)
- **`add_event` flips `bridge` on** for `RegisterKind::Url` (without the bridge
  the page has no `window.ozma.emit`). No-op for inline/dir.
- **Coexists with `.on`**: different page API (`emit` vs `call`), different
  frame kind (`ozma.emit` vs `ozma.call`), separate name maps. An event name
  and a method name never collide.

## 5. SDK internals

### 5.1 Per-handle event queues

A new structure, built at `register` time from the builder's declarations and
shared (`Arc`) between the reader thread (ingest) and the handle (drain):

```rust
// One bounded ring of raw payloads, shared (Arc) by both lookup maps.
// RingBuf = { buf: VecDeque<Value>, dropped: u64, last_warn: Option<Instant> }
type Ring = Arc<Mutex<RingBuf>>;

pub(crate) struct EventQueues {
    by_name: HashMap<String, Ring>,   // ingest routing (reader thread): wire name → ring
    by_type: HashMap<TypeId, Ring>,   // read_events: TypeId::of::<T>() → the same ring
    cap: usize,                       // default 1024
}
```

- The builder accumulates `Vec<EventDecl { name: String, type_id: TypeId }>`.
  `register` builds one `Ring` per declared event and inserts a clone of that
  `Arc` into **both** `by_name` (for the reader) and `by_type` (for
  `read_events`) — each side reaches the ring in a **single** lookup, no
  `TypeId → name → ring` double hash. Both outer maps are frozen after
  `register` (only ring *contents* mutate), so the whole `EventQueues` is shared
  as an `Arc` with **no outer lock** — only the per-ring `Mutex` is ever
  contended (the shape `HandlerRegistry` uses to install an `Arc<HashMap<…>>`
  whole).
- A bounded `crossbeam_channel` was considered (already a dependency) and
  **rejected**: its bounded channel drops the *newest* message on overflow — the
  opposite of decision #4's drop-oldest — and a one-shot `mem::take` drain beats
  N `try_recv` calls. `Mutex<VecDeque>` keeps drop-oldest atomic under one lock.
- **Deserialization is deferred to read time** (the UI thread): the reader
  thread only routes a `serde_json::Value` by name, so it stays a cheap router
  and `T`-typed work happens where `T` is known. This also localizes
  deserialize errors to `read_events`.

### 5.2 Reader thread (`session.rs`)

Add an `op=="event"` branch to the reader loop (peer of the existing `call` /
`compositing` branches):

1. Parse the line into a typed `IncomingEvent { handle, event, #[serde(default)]
   payload: Value }` added to `sdk/ratatui-ozma/src/protocol.rs`, mirroring the
   existing `IncomingCall` struct (the `call` path already parses into a typed
   struct rather than indexing raw `Value`).
2. Look up the handle's `Arc<EventQueues>` in a new
   `EventRegistry: Arc<Mutex<HashMap<String /*handle*/, Arc<EventQueues>>>>`
   (installed under the minted handle at register-reply time, exactly like
   `HandlerRegistry`); **clone the `Arc` and release the registry lock** before
   touching any ring, so a burst from one page holds the global lock only briefly.
3. `by_name.get(event)` → lock that ring, `push_back`; if `len == cap`,
   `pop_front()` (drop oldest) first. Overflow bumps a **per-ring** dropped
   counter and emits a `tracing::warn!` throttled per ring (via the ring's
   `last_warn`/`dropped` fields) so one noisy event type can't suppress warnings
   for another. An event for an unregistered name is dropped + `tracing::debug!`.

### 5.3 `read_events::<T>()` (`WebviewHandle`)

The `WebviewHandle` holds the `Arc<EventQueues>` directly (captured at
`register`), so no handle lookup is needed:

1. `ring = by_type.get(&TypeId::of::<T>())` — `None` ⇒ return `vec![]` (a single
   lookup; no `TypeId → name` indirection).
2. Lock the ring, `std::mem::take` its `VecDeque` into an owned local, then
   **release the lock**. Deserialization in step 3 must not run while the lock
   is held, or a slow/large payload would block the reader thread's ingest for
   that event type.
3. `serde_json::from_value::<T>` each payload; on `Err`, skip + `tracing::warn!`.
4. Return `Vec<T>`, oldest first.

`WebviewHandle` is `Clone` and clones share the same `Arc<EventQueues>`; an event
is consumed by whichever clone drains first (the normal case is a single
draining loop). This is documented, not guarded.

### 5.4 Reconnect

`Registration` (the replay record in `session.rs`) gains an `Arc<EventQueues>`
field alongside `handlers`, and the new `EventRegistry` threads through the same
three sites the existing shared maps already pass through: `connect()`'s
reconnect-thread closure, `spawn_reader`, and `attempt_reconnect` (the same
churn `pending_compositing` already pays). On reconnect, mirror exactly what the
handler map does at `session.rs:662-672` — `registry.remove(old_handle)` then
`registry.insert(new_handle, same_arc)` — re-installing the **same `Arc`** under
the freshly minted handle. Removing the old key matters: it stops a stale handle
entry from leaking and ensures a late `op=="event"` line addressed to the
now-defunct old handle is dropped. Buffered events survive (the `Arc` is
unchanged); the handle slot's id updates as it already does for handlers.

### 5.5 Documented limitation — instance merging

A handle may be mounted as multiple instances (`(view_id, instance_id)`). The
forwarded line carries only the **handle**, so events from *all* instances of a
handle merge into one per-handle buffer; `read_events::<T>()` has no
per-instance filter (matching the memo's no-instance API). Acceptable for
one-way notifications; noted for future work if per-instance routing is needed.

## 6. Host + page-side layers

### 6.1 `@ozma/web` (`sdk/ozma-web/src/ozma.ts`)

Add to `OzmaApi` and the `ozma` const:

```ts
/** Sends a one-way event to the host app (fire-and-forget; no reply). */
emit(event: string, payload?: unknown): void;
```

Delegates through `resolve()`; throws the same "window.ozma unavailable" error
when the bridge is absent. Returns `void`. A vitest case asserts it forwards to
the bridge with the right shape and throws when the bridge is missing.

### 6.2 Bridge JS (`src/webview/render/ozma_bridge.js`)

Add `emit` to the frozen `api` object:

```js
emit: function (event, payload) {
  cef.emit({ kind: 'ozma.emit', event: event, payload: encodeParam(payload) });
},
```

Reuses the existing top-level-`Uint8Array` tagging (`encodeParam`); no `reqId`,
no Promise, no `calls` entry, so it cannot leak. `preload.rs` gains an assertion
that the injected bundle contains `kind: 'ozma.emit'` (next to the existing
`'ozma.call'` assert).

### 6.3 Host forwarder (`src/webview/render.rs`)

A new observer on `Receive<OzmuxFrame>` gated to `kind == "ozma.emit"` (peer of
`on_ozmux_call_frame`):

1. Resolve `WebviewOwner` for `frame.webview`. Missing owner ⇒ drop +
   `tracing::debug!` (no Promise to reject).
2. `writers.send(owner.connection_id, line)` where `line` is built with **raw
   `serde_json::json!`**:
   `{"op":"event","handle":owner.handle,"event":event,"payload":payload}`.
   Building it raw (not via a typed `PushMsg` variant) keeps `PushMsg`'s `Eq`
   derive — exactly the precedent `on_ozmux_call_frame` already sets for the
   `call` line (a `Value` payload would otherwise break `Eq`).
3. No `OzmuxRpc::mint`/`note`, no reply routing — strictly fire-and-forget. A
   `writers.send` failure is dropped + `tracing::debug!`.

No change to the **host** `src/control_plane/protocol.rs`: the host builds the
line with raw `json!` (the `call`-forward precedent). The **SDK** side parses it
into a typed `IncomingEvent` struct added to `sdk/ratatui-ozma/src/protocol.rs`,
mirroring the existing `IncomingCall` — so the reader branch deserializes rather
than indexing raw `Value` (see §5.2).

## 7. Testing

- **SDK unit (`webview.rs` / `session.rs`)**:
  - `add_event` records the decl; duplicate type **and** duplicate name panic.
  - `add_event` flips `bridge` for `url`; no-op for inline.
  - reader-thread `op=="event"` routes a payload into the buffer (same paired-
    socket harness as `reader_thread_inserts_compositing_into_shared_map`).
  - `read_events` drains in order and deserializes; malformed payload skipped;
    unknown type returns `vec![]`.
  - ring drops the oldest past `cap`.
  - reconnect replays `EventQueues` under the new handle and preserves buffered
    events.
- **Host (`src/webview/render.rs`)**: a `kind:"ozma.emit"` frame produces the
  `{"op":"event",…}` line on the owner's writer; missing owner drops silently
  (paralleling the `call`-forward test).
- **Bridge JS (`preload.rs`)**: bundle contains `kind: 'ozma.emit'`.
- **`@ozma/web` (vitest)**: `emit` delegates with the right shape; throws when
  the bridge is absent.
- **Integration (`tests/integration.rs`)**: end-to-end page emit → `read_events`
  over a paired socket.
- **Example (`examples/ratatui_webview.rs`)**: extend with an `<input>` that
  calls `window.ozma.emit("hello", …)` and an app loop that renders the latest
  received message, demonstrating the round trip.

## 8. Out of scope

- Per-instance event routing (see §5.5).
- Configurable per-event buffer caps (the 1024 default is fixed for now; the
  `cap` field leaves room to expose it later).
- Backpressure signalling to the page (drops are silent to the page; only the
  app sees the throttled warn).
- Any change to the `call` RPC path or the app→page `emit` path.
