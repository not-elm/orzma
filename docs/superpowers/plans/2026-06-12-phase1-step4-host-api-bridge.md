# Phase 1 Step 4: Host-API Bridge (`window.<ns>.<method>`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make an OSC-mounted extension webview call `window.<ns>.<method>(...args)` and have it run `api[ns][method]` in the single Node host, gated by the surface's capabilities, with the result (incl. binary) returned to that webview's Promise.

**Architecture:** Spec `docs/superpowers/specs/2026-06-11-phase1-single-host-process-design.md` §4③ (bridge-only slice; legacy stays until Step 5, memo+E2E is Step 6). A new Rust NDJSON RPC client (`HostRpcClient`, Tokio-free `UnixStream` + writer/reader threads, mirroring `host.rs`) dials the host's `OZMUX_HOST_RPC_SOCK`. A **second observer** on the existing single `Receive<OzmuxFrame>` event (NOT a second `JsEmitEventPlugin` — the raw IPC receiver is shared and `try_recv`-consumed) discriminates `kind:"host.call"` frames, gates them against the webview entity's `GrantedNamespaces` (the trusted key — never the JS payload), rewrites the page-local `reqId` to a globally-unique id (avoiding cross-webview `reqId` collisions), forwards to the host, and routes the reply back via `HostEmitEvent` on the existing `"ozmux"` channel. New-model surfaces get a new injected Proxy bridge (`host_bridge.js`) instead of the legacy `ozmux.js`; binary uses the `{__u8}` boundary tag (webview-side codec mirrors the host's vitest-tested `binary-codec.ts`).

**Tech Stack:** Rust (Bevy 0.18 observers/resources, `std::os::unix::net::UnixStream`, `crossbeam-channel` internal to the host crate), TypeScript/Node host (`host/src/rpc-server.ts` + esbuild bundle), hand-written injected JS (`include_str!`, mirroring the existing `ozmux.js`).

**Key facts (verified against current code):**
- The host RPC server speaks **NDJSON** (newline-delimited JSON), reads/writes lines, caps an unframed inbound flood at 8 MiB, and has **no response-size cap** (`host/src/rpc-server.ts:7,38-44,82-93`). Result frames are `{reqId, ok:true, value}` / `{reqId, ok:false, error}` (`host/src/dispatch.ts:13-15`). Args are `{__u8}`-decoded before dispatch; results `{__u8}`-encoded after (`host/src/binary-codec.ts:20-37`).
- Rust does **not** yet connect to the RPC socket — `HostProcess` only stores `rpc_sock_path()` (`crates/extension_host/src/host_process.rs:38,120-122`). The Tokio-free blocking-socket + crossbeam + thread-lifecycle pattern to mirror lives in `crates/extension_host/src/host.rs`.
- `Receive<M>` is a broadcast `EntityEvent` (`#[event_target] webview: Entity`, `payload: M`) (`bevy_cef/src/common/ipc/js_emit.rs:8-12`); `HostEmitEvent` is an `EntityEvent` with `webview`/`id`/`payload: String`, built via `HostEmitEvent::new(webview, id, &impl Serialize)` (`bevy_cef/.../host_emit.rs:13-29`). There is exactly **one** `JsEmitEventPlugin::<OzmuxFrame>` and one `receive_events` that `try_recv`-consumes the shared `IpcEventRawReceiver` — adding a **second plugin** would steal messages, but a **second observer** on the produced `Receive<OzmuxFrame>` is a safe broadcast.
- `OzmuxFrame` is `#[serde(transparent)] struct OzmuxFrame(serde_json::Value)` (`src/extension_render.rs:35-37`), so it matches any JSON object — both observers must branch on `kind`.
- `GrantedNamespaces(pub(crate) HashSet<String>)` is stamped on the surface (== webview) entity at OSC-mount (`src/osc_webview.rs:37-43,105`), currently `#[allow(dead_code)]` with no production reader.
- `cef.emit(arg)` serializes only its first argument (single self-describing object; no channel arg) — `src/extension_render/ozmux.js:1-4`.

**Verification commands:**
- Host-crate Rust tests: `cargo test -p ozmux_extension_host`
- Binary (gui) Rust tests: `cargo test -p ozmux-gui --lib -- --test-threads=1` (the bridge/observer tests live here)
- Host (Node) tests: `pnpm -C host test`
- Full build: `cargo build`
- Lint/format: `cargo clippy --workspace --all-targets && cargo fmt --check`
- NOTE: a full `cargo test` has a pre-existing IME failure + a parallel-teardown SIGSEGV unrelated to this work; use the per-crate commands above as the primary signal, and `--test-threads=1` for the gui crate.

---

## File Structure

**Create:**
- `crates/extension_host/src/rpc_client.rs` — Tokio-free NDJSON RPC client (`HostRpcClient`): one long-lived `UnixStream`, a writer thread draining outbound request lines, a reader thread pumping inbound reply lines onto a crossbeam channel. Non-crossbeam public surface (`connect` / `send_line` / `try_recv_response`) so the binary crate never names a crossbeam type. Fully unit-testable against a fake `UnixListener`.
- `src/extension_render/host_bridge.js` — the new injected bridge: builds one `window[ns]` Proxy per granted namespace (read from `window.__ozmuxGranted`), `cef.emit({kind:"host.call", ...})`, correlates replies on the `"ozmux"` channel, and applies the `{__u8}` codec at the argument/result boundary.

**Modify:**
- `crates/extension_host/src/lib.rs` — `pub mod rpc_client;` + `pub use rpc_client::HostRpcClient;`.
- `host/src/rpc-server.ts` — add a response-size cap (`MAX_RPC_RESULT_BYTES`) before writing a reply; oversize → error frame. Then rebuild `assets/host.mjs`.
- `src/extension_render.rs` — add the `HostRpc` resource, a new `on_host_call_frame` observer (capability gate + RPC forward + reqId rewrite), a `drain_host_rpc_responses` system (reply → originating webview), a `kind:"host.call"` guard on the legacy `on_ozmux_frame`, host-bridge injection in `finish_extension_setup` for surfaces carrying `GrantedNamespaces`, and inflight pruning on despawn.
- `src/extension_manager.rs` — connect `HostRpcClient` on `LifecycleEvent::Ready`, store it into `HostRpc`; clear on `Exited`/`SpawnFailed`.
- `src/osc_webview.rs` — remove the now-stale `#[allow(dead_code)]` on `GrantedNamespaces` (Task 3 adds the first production reader).

---

## Task 1: `HostRpcClient` — NDJSON RPC client to the host socket

**Files:**
- Create: `crates/extension_host/src/rpc_client.rs`
- Modify: `crates/extension_host/src/lib.rs`
- Test: inline `#[cfg(test)] mod tests` in `rpc_client.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/extension_host/src/rpc_client.rs` with ONLY the test module first (so the build fails on the missing type):

```rust
//! Tokio-free NDJSON RPC client to the single Node host: a long-lived
//! `UnixStream` with a writer thread draining outbound request lines and a
//! reader thread pumping inbound NDJSON reply lines onto a crossbeam channel.

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    use std::time::Duration;

    #[test]
    fn round_trips_one_ndjson_request_and_reply() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("rpc.sock");
        let listener = UnixListener::bind(&sock).unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            assert!(line.contains("\"reqId\":\"7\""), "server saw the request line");
            let mut w = stream;
            w.write_all(b"{\"reqId\":\"7\",\"ok\":true,\"value\":42}\n").unwrap();
            w.flush().unwrap();
        });

        let client = HostRpcClient::connect(&sock).unwrap();
        client.send_line("{\"reqId\":\"7\",\"ns\":\"fs\",\"method\":\"read\",\"args\":[]}".to_string());

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let reply = loop {
            if let Some(line) = client.try_recv_response() {
                break line;
            }
            assert!(std::time::Instant::now() < deadline, "no reply within 2s");
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(reply, "{\"reqId\":\"7\",\"ok\":true,\"value\":42}");
        server.join().unwrap();
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ozmux_extension_host rpc_client`
Expected: FAIL to compile — `HostRpcClient` not found (and `rpc_client` module not declared yet).

- [ ] **Step 3: Write the implementation**

Prepend the implementation ABOVE the test module in `crates/extension_host/src/rpc_client.rs`:

```rust
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, unbounded};
use std::io::{BufRead, BufReader, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

const WRITER_POLL: Duration = Duration::from_millis(100);

/// A connected NDJSON RPC client to the single Node host. Outbound
/// `{reqId, ns, method, args}` request lines are queued via
/// [`HostRpcClient::send_line`]; inbound `{reqId, ok, …}` reply lines are read
/// via [`HostRpcClient::try_recv_response`]. The writer/reader threads are
/// joined on drop.
pub struct HostRpcClient {
    outbound: Sender<String>,
    responses: Receiver<String>,
    shutdown: Arc<AtomicBool>,
    stream: UnixStream,
    writer: Option<JoinHandle<()>>,
    reader: Option<JoinHandle<()>>,
}

impl HostRpcClient {
    /// Connects to the host RPC socket and starts the writer + reader threads.
    pub fn connect(sock: &Path) -> std::io::Result<Self> {
        let stream = UnixStream::connect(sock)?;
        let mut write_half = stream.try_clone()?;
        let read_half = stream.try_clone()?;
        let (out_tx, out_rx) = unbounded::<String>();
        let (in_tx, in_rx) = unbounded::<String>();
        let shutdown = Arc::new(AtomicBool::new(false));

        let writer = {
            let shutdown = Arc::clone(&shutdown);
            std::thread::spawn(move || {
                loop {
                    match out_rx.recv_timeout(WRITER_POLL) {
                        Ok(line) => {
                            if write_half.write_all(line.as_bytes()).is_err()
                                || write_half.write_all(b"\n").is_err()
                                || write_half.flush().is_err()
                            {
                                break;
                            }
                        }
                        Err(RecvTimeoutError::Timeout) => {
                            if shutdown.load(Ordering::SeqCst) {
                                break;
                            }
                        }
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
            })
        };

        let reader = std::thread::spawn(move || {
            let mut lines = BufReader::new(read_half);
            let mut buf = String::new();
            loop {
                buf.clear();
                match lines.read_line(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let line = buf.trim_end_matches(['\n', '\r']).to_string();
                        if in_tx.send(line).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        Ok(Self {
            outbound: out_tx,
            responses: in_rx,
            shutdown,
            stream,
            writer: Some(writer),
            reader: Some(reader),
        })
    }

    /// Queues an NDJSON request line for the host (the writer appends `\n`).
    pub fn send_line(&self, line: String) {
        let _ = self.outbound.send(line);
    }

    /// Pops the next NDJSON reply line, or `None` if none is queued.
    pub fn try_recv_response(&self) -> Option<String> {
        self.responses.try_recv().ok()
    }
}

impl Drop for HostRpcClient {
    fn drop(&mut self) {
        // NOTE: signal shutdown, then shut the stream so the reader's blocking
        // read_line returns; the writer exits within one WRITER_POLL even though
        // a sender clone may still live in the HostRpc resource.
        self.shutdown.store(true, Ordering::SeqCst);
        let _ = self.stream.shutdown(Shutdown::Both);
        if let Some(w) = self.writer.take() {
            let _ = w.join();
        }
        if let Some(r) = self.reader.take() {
            let _ = r.join();
        }
    }
}
```

Then declare + re-export the module. In `crates/extension_host/src/lib.rs`, add `pub mod rpc_client;` to the module list (alphabetical, after `protocol;` / before `registry;` is fine) and add to the re-export block:

```rust
pub use rpc_client::HostRpcClient;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p ozmux_extension_host rpc_client`
Expected: PASS (1 test).

- [ ] **Step 5: Lint + format**

Run: `cargo clippy -p ozmux_extension_host --all-targets && cargo fmt`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/extension_host/src/rpc_client.rs crates/extension_host/src/lib.rs
git commit -m "feat(extension_host): NDJSON HostRpcClient to the single host socket"
```

---

## Task 2: Response-size guard in the host RPC server

**Files:**
- Modify: `host/src/rpc-server.ts`
- Modify (generated): `assets/host.mjs` (rebuilt)
- Test: `host/src/rpc-server.test.ts`

Rationale for the cap value: the `ozmux-ext://` asset scheme already serves large static files directly from Rust (up to 64 MiB), so the RPC channel is for dynamic/structured calls and is capped tighter at **8 MiB**, matching the existing inbound `MAX_RPC_LINE_BYTES`. Large bytes should flow over the asset scheme, not RPC.

- [ ] **Step 1: Write the failing test**

Append to `host/src/rpc-server.test.ts` inside the `describe('bindHostRpcServer', …)` block (the file already provides `connect()`, `rpc()`, `sockPath`, and `server` teardown):

```ts
  it('rejects an over-sized result with an error frame instead of writing it', async () => {
    const bigApi: ApiNamespaceMap = {
      big: { blob: async () => 'x'.repeat(9 * 1024 * 1024) },
    };
    server = await bindHostRpcServer(sockPath, bigApi);
    const s = await connect();
    const reply = await rpc(s, { reqId: '1', ns: 'big', method: 'blob', args: [] });
    s.end();
    expect(reply).toEqual({ reqId: '1', ok: false, error: 'host result exceeds max size' });
  });
```

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm -C host test -- rpc-server`
Expected: FAIL — the server currently writes the 9 MiB success frame, so `reply` is `{reqId:'1', ok:true, value:'xxxx…'}`, not the error frame.

- [ ] **Step 3: Write the implementation**

In `host/src/rpc-server.ts`, add the cap constant under the existing one (line 7):

```ts
const MAX_RPC_LINE_BYTES = 8 * 1024 * 1024;
const MAX_RPC_RESULT_BYTES = 8 * 1024 * 1024;
```

Then replace the success-write arm of the `dispatchHostCall(...)` chain (currently `.then((result) => conn.write(\`${JSON.stringify(result)}\n\`))`) with a size check:

```ts
    dispatchHostCall(api, frame)
      .then((result) => {
        const line = JSON.stringify(result);
        if (Buffer.byteLength(line, 'utf8') > MAX_RPC_RESULT_BYTES) {
          // NOTE: a single oversized JSON string would choke the render process
          // crossing the CEF IPC boundary; reject with an addressable error
          // frame so the caller's reqId Promise settles. Big bytes belong on the
          // ozmux-ext:// asset scheme, not RPC.
          conn.write(
            `${JSON.stringify({ reqId: frame.reqId, ok: false, error: 'host result exceeds max size' })}\n`,
          );
          return;
        }
        conn.write(`${line}\n`);
      })
      .catch((err) => {
        // NOTE: dispatchHostCall is contracted never to reject; reply with an
        // error frame anyway so a future regression cannot leave the caller's
        // reqId Promise hanging forever.
        console.error('host rpc: dispatch threw', err);
        conn.write(
          `${JSON.stringify({ reqId: frame.reqId, ok: false, error: 'internal host error' })}\n`,
        );
      });
```

- [ ] **Step 4: Run test to verify it passes**

Run: `pnpm -C host test -- rpc-server`
Expected: PASS (all rpc-server tests, including the new one).

- [ ] **Step 5: Rebuild the embedded bundle + typecheck**

Run: `pnpm -C host build && pnpm -C host check-types`
Expected: `assets/host.mjs` regenerated; types clean.

- [ ] **Step 6: Commit**

```bash
git add host/src/rpc-server.ts host/src/rpc-server.test.ts assets/host.mjs
git commit -m "feat(host): cap RPC result frames at 8 MiB with an error reply"
```

---

## Task 3: `HostRpc` resource + capability-gated host-call observer

**Files:**
- Modify: `src/extension_render.rs`
- Modify: `src/osc_webview.rs`
- Test: inline `#[cfg(test)] mod tests` in `src/extension_render.rs`

This adds the new path **alongside** the legacy one: a second observer on the shared `Receive<OzmuxFrame>` event, gating on the trusted `frame.webview` entity's `GrantedNamespaces`.

- [ ] **Step 1: Add imports + the `HostRpc` resource**

In `src/extension_render.rs` import block (top of file, no blank lines between groups), add:

```rust
use crate::osc_webview::GrantedNamespaces;
use ozmux_extension_host::HostRpcClient;
use serde_json::Value;
```

Add the resource + its helpers near the other resources (after `WebviewSurfaceIdMap`, before `WebviewMountUnresolved`). `HostRpc` is `pub(crate)` ONLY because `extension_manager::poll_host_lifecycle` populates it; its fields stay private:

```rust
/// The connected host RPC client plus the in-flight `globalReqId → (webview,
/// pageReqId)` correlation. `globalReqId` is minted Rust-side (a monotonic
/// counter) so page-local `reqId`s — which collide across webviews — are never
/// used as a routing key. Populated by `extension_manager` on host readiness.
#[derive(Resource, Default)]
pub(crate) struct HostRpc {
    client: Option<HostRpcClient>,
    inflight: HashMap<String, (Entity, String)>,
    next_id: u64,
}

impl HostRpc {
    /// Installs a freshly-connected client, clearing any stale correlation /
    /// counter from a previous host generation.
    pub(crate) fn set_client(&mut self, client: HostRpcClient) {
        self.client = Some(client);
        self.inflight.clear();
        self.next_id = 0;
    }

    /// Drops the client (host exited): subsequent calls reject `host_unavailable`.
    pub(crate) fn clear_client(&mut self) {
        self.client = None;
        self.inflight.clear();
    }
}
```

- [ ] **Step 2: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block in `src/extension_render.rs`. These exercise the gate without any socket (denied / unavailable paths) and via a fake drain server (allowed path):

```rust
    use crate::osc_webview::GrantedNamespaces;
    use bevy_cef::prelude::HostEmitEvent;
    use std::collections::HashSet;

    #[derive(Resource, Default)]
    struct CapturedEmits(Vec<(Entity, String)>);

    fn capture_emits(ev: On<HostEmitEvent>, mut cap: ResMut<CapturedEmits>) {
        cap.0.push((ev.webview, ev.payload.clone()));
    }

    fn gate_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<HostRpc>();
        app.init_resource::<CapturedEmits>();
        app.add_observer(on_host_call_frame);
        app.add_observer(capture_emits);
        app
    }

    fn host_call(req_id: &str, ns: &str, method: &str) -> OzmuxFrame {
        OzmuxFrame(serde_json::json!({
            "kind": "host.call", "reqId": req_id, "ns": ns, "method": method, "args": []
        }))
    }

    #[test]
    fn host_call_denied_for_ungranted_namespace_is_not_forwarded() {
        let mut app = gate_app();
        let mut caps = HashSet::new();
        caps.insert("clipboard".to_string());
        let webview = app.world_mut().spawn(GrantedNamespaces(caps)).id();

        app.world_mut().trigger(Receive {
            webview,
            payload: host_call("h0", "fs", "read"),
        });

        assert!(
            app.world().resource::<HostRpc>().inflight.is_empty(),
            "a denied call must NOT be forwarded (no in-flight entry)"
        );
        let cap = app.world().resource::<CapturedEmits>();
        assert_eq!(cap.0.len(), 1, "exactly one reject emitted");
        assert_eq!(cap.0[0].0, webview);
        assert!(cap.0[0].1.contains("capability_denied"), "rejected as capability_denied");
        assert!(cap.0[0].1.contains("\"reqId\":\"h0\""), "reply carries the page-local reqId");
    }

    #[test]
    fn host_call_trust_key_is_the_webview_entity_not_the_payload() {
        // A granted entity exists, but the CALLER entity is not granted; a
        // spoofed surfaceId/granted hint in the payload must not help.
        let mut app = gate_app();
        let mut caps = HashSet::new();
        caps.insert("fs".to_string());
        let _granted = app.world_mut().spawn(GrantedNamespaces(caps)).id();
        let caller = app.world_mut().spawn(GrantedNamespaces(HashSet::new())).id();

        app.world_mut().trigger(Receive {
            webview: caller,
            payload: OzmuxFrame(serde_json::json!({
                "kind": "host.call", "reqId": "h0", "ns": "fs", "method": "read",
                "args": [], "surfaceId": "spoofed", "granted": ["fs"]
            })),
        });

        assert!(app.world().resource::<HostRpc>().inflight.is_empty());
        let cap = app.world().resource::<CapturedEmits>();
        assert_eq!(cap.0.len(), 1);
        assert!(cap.0[0].1.contains("capability_denied"));
    }

    #[test]
    fn host_call_rejects_when_host_unavailable() {
        let mut app = gate_app(); // HostRpc::default → client None
        let mut caps = HashSet::new();
        caps.insert("fs".to_string());
        let webview = app.world_mut().spawn(GrantedNamespaces(caps)).id();

        app.world_mut().trigger(Receive {
            webview,
            payload: host_call("h0", "fs", "read"),
        });

        assert!(app.world().resource::<HostRpc>().inflight.is_empty());
        let cap = app.world().resource::<CapturedEmits>();
        assert_eq!(cap.0.len(), 1);
        assert!(cap.0[0].1.contains("host_unavailable"));
    }

    #[test]
    fn host_call_for_granted_namespace_is_forwarded_and_tracked() {
        use std::io::{BufRead, BufReader};
        use std::os::unix::net::UnixListener;

        // A fake host that accepts + drains forwarded lines (never replies).
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("rpc.sock");
        let listener = UnixListener::bind(&sock).unwrap();
        let server = std::thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                let mut r = BufReader::new(stream);
                let mut line = String::new();
                while r.read_line(&mut line).map(|n| n > 0).unwrap_or(false) {
                    line.clear();
                }
            }
        });

        let mut app = gate_app();
        let client = ozmux_extension_host::HostRpcClient::connect(&sock).unwrap();
        app.world_mut().resource_mut::<HostRpc>().set_client(client);

        let mut caps = HashSet::new();
        caps.insert("fs".to_string());
        let webview = app.world_mut().spawn(GrantedNamespaces(caps)).id();

        app.world_mut().trigger(Receive {
            webview,
            payload: host_call("h0", "fs", "read"),
        });

        let hr = app.world().resource::<HostRpc>();
        assert_eq!(hr.inflight.len(), 1, "an allowed call is tracked in-flight");
        let entry = hr.inflight.values().next().unwrap();
        assert_eq!(entry.0, webview);
        assert_eq!(entry.1.as_str(), "h0", "in-flight maps the global id back to the page-local reqId");
        assert!(
            app.world().resource::<CapturedEmits>().0.is_empty(),
            "an allowed call is forwarded, not rejected"
        );

        // Drop the client so the fake server's read loop ends and the thread joins.
        app.world_mut().resource_mut::<HostRpc>().clear_client();
        let _ = server.join();
    }
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p ozmux-gui --lib -- --test-threads=1 host_call`
Expected: FAIL to compile — `on_host_call_frame` not defined.

- [ ] **Step 4: Write the observer + helpers**

In `src/extension_render.rs`, add the new observer (place it just after `on_ozmux_frame`, before the outbound `drain_handler_responses`). Per the parameter-ordering rule the `On<…>` trigger is fixed first, then mutable params:

```rust
/// Inbound (new-model host API): a `window.<ns>.<method>` call arrives as a
/// `Receive<OzmuxFrame>` with `kind:"host.call"`. The trusted caller is
/// `frame.webview` (bound per-webview by `bevy_cef`, never the JS payload); its
/// `GrantedNamespaces` decides whether the call may proceed. Allowed calls are
/// forwarded to the single host over a Rust-minted global `reqId`; denied or
/// host-down calls reject the page-local Promise directly.
///
/// Runs as a SECOND observer on the shared `Receive<OzmuxFrame>` event (NOT a
/// second `JsEmitEventPlugin`): observers are broadcast, so the legacy
/// `on_ozmux_frame` still fires for the same frame and ignores `host.call`.
fn on_host_call_frame(
    frame: On<Receive<OzmuxFrame>>,
    mut commands: Commands,
    mut host_rpc: ResMut<HostRpc>,
    granted: Query<&GrantedNamespaces>,
) {
    let payload = &frame.payload.0;
    if payload.get("kind").and_then(Value::as_str) != Some("host.call") {
        return;
    }
    let webview = frame.webview;
    let req_id = payload.get("reqId").and_then(Value::as_str).unwrap_or_default();
    let ns = payload.get("ns").and_then(Value::as_str).unwrap_or_default();
    let method = payload.get("method").and_then(Value::as_str).unwrap_or_default();

    let allowed = granted
        .get(webview)
        .map(|g| g.0.contains(ns))
        .unwrap_or(false);
    if !allowed {
        reject_host_call(&mut commands, webview, req_id, &format!("capability_denied: {ns}"));
        return;
    }
    if host_rpc.client.is_none() {
        reject_host_call(&mut commands, webview, req_id, "host_unavailable");
        return;
    }

    let global_id = host_rpc.next_id.to_string();
    host_rpc.next_id += 1;
    let args = payload.get("args").cloned().unwrap_or_else(|| Value::Array(Vec::new()));
    let line = serde_json::json!({
        "reqId": global_id, "ns": ns, "method": method, "args": args
    })
    .to_string();
    if let Some(client) = host_rpc.client.as_ref() {
        client.send_line(line);
    }
    host_rpc
        .inflight
        .insert(global_id, (webview, req_id.to_string()));
}

/// Emits a `{reqId, ok:false, error}` reply to a single webview on the `"ozmux"`
/// channel (shared with the legacy outbound), settling the page-local Promise.
fn reject_host_call(commands: &mut Commands, webview: Entity, req_id: &str, error: &str) {
    let payload = serde_json::json!({ "reqId": req_id, "ok": false, "error": error });
    commands.trigger(HostEmitEvent::new(webview, "ozmux", &payload));
}
```

Add the guard at the TOP of the legacy `on_ozmux_frame` body (right after `let webview = frame.webview;`), so a `host.call` frame is not mistaken for a legacy handler frame:

```rust
    let webview = frame.webview;
    if frame.payload.0.get("kind").and_then(serde_json::Value::as_str) == Some("host.call") {
        return;
    }
```

Register the resource + observer in `OzmuxExtensionRenderPlugin::build` (add to the existing chain):

```rust
        app.add_plugins(JsEmitEventPlugin::<OzmuxFrame>::default())
            .init_resource::<ExtensionHandlersBridge>()
            .init_resource::<WebviewSurfaceIdMap>()
            .init_resource::<HostRpc>()
            .add_observer(on_ozmux_frame)
            .add_observer(on_host_call_frame)
            .add_observer(prune_webview_id_map_on_remove)
```

Finally, in `src/osc_webview.rs`, remove the now-stale dead-code suppression on `GrantedNamespaces` (Task 3 is the production reader the NOTE anticipated). Delete these lines (osc_webview.rs:38-42):

```rust
// NOTE: the production (non-test) build has no reader yet, so dead_code fires;
// #[expect] cannot suppress it because the test binary DOES read the field
// (the expectation would then fail under cfg(test)). Remove when the host-API
// bridge (Step 3) adds a non-test reader.
#[allow(dead_code)]
```

so the struct becomes:

```rust
#[derive(Component, Debug, Clone, Default)]
pub(crate) struct GrantedNamespaces(pub(crate) HashSet<String>);
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p ozmux-gui --lib -- --test-threads=1 host_call`
Expected: PASS (4 new tests). Also run the existing suite to confirm no regression: `cargo test -p ozmux-gui --lib -- --test-threads=1 extension_render`.

- [ ] **Step 6: Lint + format**

Run: `cargo clippy -p ozmux-gui --all-targets && cargo fmt`
Expected: clean (no `dead_code` warning for `GrantedNamespaces`).

- [ ] **Step 7: Commit**

```bash
git add src/extension_render.rs src/osc_webview.rs
git commit -m "feat(extension_render): capability-gated host-call observer + HostRpc"
```

---

## Task 4: Route host replies back + prune in-flight on despawn

**Files:**
- Modify: `src/extension_render.rs`
- Test: inline `#[cfg(test)] mod tests` in `src/extension_render.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block. A fake host that replies once lets `drain_host_rpc_responses` route the reply back to the originating webview with the page-local `reqId`:

```rust
    #[test]
    fn host_reply_routed_back_to_origin_with_page_local_req_id() {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixListener;
        use std::time::Duration;

        // Fake host: read one forwarded line, reply keyed on the SAME global id.
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("rpc.sock");
        let listener = UnixListener::bind(&sock).unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let frame: serde_json::Value = serde_json::from_str(&line).unwrap();
            let gid = frame.get("reqId").and_then(|v| v.as_str()).unwrap().to_string();
            let mut w = stream;
            w.write_all(
                format!("{{\"reqId\":\"{gid}\",\"ok\":true,\"value\":\"hi\"}}\n").as_bytes(),
            )
            .unwrap();
            w.flush().unwrap();
        });

        let mut app = gate_app(); // already has on_host_call_frame + capture_emits + CapturedEmits
        app.add_systems(Update, drain_host_rpc_responses);
        let client = ozmux_extension_host::HostRpcClient::connect(&sock).unwrap();
        app.world_mut().resource_mut::<HostRpc>().set_client(client);

        let mut caps = HashSet::new();
        caps.insert("fs".to_string());
        let webview = app.world_mut().spawn(GrantedNamespaces(caps)).id();

        // Forward an allowed call (server replies), then drain until routed back.
        app.world_mut().trigger(Receive {
            webview,
            payload: host_call("h0", "fs", "read"),
        });

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            app.update();
            if !app.world().resource::<CapturedEmits>().0.is_empty() {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "reply never routed back");
            std::thread::sleep(Duration::from_millis(5));
        }

        let cap = app.world().resource::<CapturedEmits>();
        assert_eq!(cap.0[0].0, webview, "reply targets the originating webview");
        assert!(cap.0[0].1.contains("\"reqId\":\"h0\""), "page-local reqId restored");
        assert!(cap.0[0].1.contains("\"value\":\"hi\""), "value forwarded through");
        assert!(
            app.world().resource::<HostRpc>().inflight.is_empty(),
            "the in-flight entry is consumed on reply"
        );

        app.world_mut().resource_mut::<HostRpc>().clear_client();
        let _ = server.join();
    }

    #[test]
    fn pruning_drops_in_flight_calls_for_a_despawned_surface() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<HostRpc>();
        app.init_resource::<ExtensionHandlersBridge>();
        app.init_resource::<WebviewSurfaceIdMap>();
        app.add_observer(prune_webview_id_map_on_remove);

        let surface = app.world_mut().spawn(SurfaceMarker).id();
        app.world_mut()
            .resource_mut::<HostRpc>()
            .inflight
            .insert("0".to_string(), (surface, "h0".to_string()));

        app.world_mut().entity_mut(surface).despawn();

        assert!(
            app.world().resource::<HostRpc>().inflight.is_empty(),
            "despawning a surface drops its in-flight host calls"
        );
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ozmux-gui --lib -- --test-threads=1 host_reply pruning_drops`
Expected: FAIL to compile — `drain_host_rpc_responses` not defined; `prune_webview_id_map_on_remove` does not yet touch `HostRpc`.

- [ ] **Step 3: Write the drain system + extend pruning**

Add the drain system in `src/extension_render.rs` (after `drain_handler_responses`):

```rust
/// Outbound (new-model host API): drains the host's NDJSON reply lines, maps the
/// Rust-minted global `reqId` back to its `(webview, pageReqId)`, restores the
/// page-local `reqId`, and re-emits each reply to the originating webview on the
/// `"ozmux"` channel. A reply with no live in-flight entry (surface despawned)
/// is dropped.
fn drain_host_rpc_responses(mut commands: Commands, mut host_rpc: ResMut<HostRpc>) {
    let mut lines = Vec::new();
    if let Some(client) = host_rpc.client.as_ref() {
        while let Some(line) = client.try_recv_response() {
            lines.push(line);
        }
    }
    for line in lines {
        let Ok(mut frame) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(global_id) = frame.get("reqId").and_then(Value::as_str).map(str::to_owned) else {
            continue;
        };
        let Some((webview, local_id)) = host_rpc.inflight.remove(&global_id) else {
            continue;
        };
        frame["reqId"] = Value::String(local_id);
        commands.trigger(HostEmitEvent::new(webview, "ozmux", &frame));
    }
}
```

Register it in `OzmuxExtensionRenderPlugin::build`'s `Update` systems tuple (add alongside `drain_handler_responses`):

```rust
            .add_systems(
                Update,
                (
                    finish_extension_setup.in_set(OzmuxSystems::SetupSurface),
                    drain_handler_responses,
                    drain_host_rpc_responses,
                    sync_focused_webview.after(OzmuxSystems::Input),
                ),
            );
```

Extend `prune_webview_id_map_on_remove` to also drop in-flight host calls for the removed entity. Update its signature (mutable params first) and body:

```rust
fn prune_webview_id_map_on_remove(
    ev: On<Remove, SurfaceMarker>,
    mut map: ResMut<WebviewSurfaceIdMap>,
    mut host_rpc: ResMut<HostRpc>,
    bridge: Res<ExtensionHandlersBridge>,
    ids: Query<&ExtensionSurfaceId>,
) {
    if let Ok(id) = ids.get(ev.entity) {
        map.0.remove(&id.0);
        bridge.0.disconnect(&id.0);
    }
    host_rpc.inflight.retain(|_, (entity, _)| *entity != ev.entity);
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ozmux-gui --lib -- --test-threads=1 host_reply pruning_drops`
Expected: PASS (2 tests). Re-run the prune regression too: `cargo test -p ozmux-gui --lib -- --test-threads=1 prune_webview_id_map_removes_entry_on_surface_despawn`.

- [ ] **Step 5: Lint + format + commit**

```bash
cargo clippy -p ozmux-gui --all-targets && cargo fmt
git add src/extension_render.rs
git commit -m "feat(extension_render): route host replies back + prune in-flight on despawn"
```

---

## Task 5: Connect the RPC client on host readiness

**Files:**
- Modify: `src/extension_manager.rs`
- Test: inline `#[cfg(test)] mod tests` in `src/extension_manager.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `src/extension_manager.rs` a focused test of the resource's reconnect semantics (no socket needed — `clear_client` resets correlation so a host restart cannot resolve a stale global id against a new webview):

```rust
    #[test]
    fn clearing_the_host_client_drops_stale_in_flight_correlation() {
        use crate::extension_render::HostRpc;
        let mut hr = HostRpc::default();
        hr.note_in_flight_for_test("0", bevy::prelude::Entity::PLACEHOLDER, "h0");
        assert!(hr.has_in_flight_for_test());
        hr.clear_client();
        assert!(!hr.has_in_flight_for_test(), "clear_client wipes stale correlation");
    }
```

This requires tiny `pub(crate)` test seams on `HostRpc` (its fields are private). Add them to `src/extension_render.rs` under the existing `impl HostRpc` (place AFTER `set_client`/`clear_client` to keep the public-ish surface first):

```rust
    #[cfg(test)]
    pub(crate) fn note_in_flight_for_test(&mut self, global_id: &str, webview: Entity, local: &str) {
        self.inflight.insert(global_id.to_string(), (webview, local.to_string()));
    }

    #[cfg(test)]
    pub(crate) fn has_in_flight_for_test(&self) -> bool {
        !self.inflight.is_empty()
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ozmux-gui --lib -- --test-threads=1 clearing_the_host_client`
Expected: FAIL to compile — `note_in_flight_for_test` / `has_in_flight_for_test` not defined. (This is a resource-invariant test for `clear_client`; the production wiring it accompanies — connect-on-ready — is verified by `cargo build` type-checking in Step 4 and end-to-end in Step 6, since a live `node` host is impractical to unit-test here.)

- [ ] **Step 3: Wire connect-on-ready into `poll_host_lifecycle`**

In `src/extension_manager.rs`, extend the host imports — add `HostRpcClient` to the `ozmux_extension_host` use list and import `HostRpc`:

```rust
use crate::extension_render::HostRpc;
```

and add `HostRpcClient` to the existing `use ozmux_extension_host::{…}` list.

Replace `poll_host_lifecycle` (currently `fn poll_host_lifecycle(host: Option<Res<HostRuntime>>)`) with a version that owns the RPC lifecycle (mutable param first; `Option<ResMut<HostRpc>>` keeps manager-only tests panic-free when the render plugin is absent):

```rust
fn poll_host_lifecycle(mut host_rpc: Option<ResMut<HostRpc>>, host: Option<Res<HostRuntime>>) {
    let Some(host) = host else {
        return;
    };
    while let Ok(event) = host.host.events().try_recv() {
        match event {
            LifecycleEvent::Ready => match HostRpcClient::connect(host.host.rpc_sock_path()) {
                Ok(client) => {
                    tracing::info!("single host process ready; RPC connected");
                    if let Some(hr) = host_rpc.as_mut() {
                        hr.set_client(client);
                    }
                }
                Err(error) => {
                    tracing::error!(%error, "single host ready but RPC connect failed")
                }
            },
            LifecycleEvent::SpawnFailed { error } => {
                tracing::error!(%error, "single host failed to become ready");
                if let Some(hr) = host_rpc.as_mut() {
                    hr.clear_client();
                }
            }
            LifecycleEvent::Exited { status } => {
                tracing::warn!(?status, "single host process exited");
                if let Some(hr) = host_rpc.as_mut() {
                    hr.clear_client();
                }
            }
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ozmux-gui --lib -- --test-threads=1 clearing_the_host_client`
Expected: PASS. Then `cargo build` to confirm the manager wiring type-checks.

- [ ] **Step 5: Lint + format + commit**

```bash
cargo clippy --workspace --all-targets && cargo fmt
git add src/extension_manager.rs src/extension_render.rs
git commit -m "feat(extension_manager): connect HostRpcClient on host readiness"
```

---

## Task 6: Inject the `host_bridge.js` Proxy for new-model surfaces

**Files:**
- Create: `src/extension_render/host_bridge.js`
- Modify: `src/extension_render.rs`
- Test: inline `#[cfg(test)] mod tests` in `src/extension_render.rs`

- [ ] **Step 1: Write the bridge JS**

Create `src/extension_render/host_bridge.js` (hand-written + `include_str!`d, mirroring the `ozmux.js` convention; the webview-side `{__u8}` codec mirrors the vitest-tested `host/src/binary-codec.ts` and is verified end-to-end in Step 6):

```js
// NOTE: bevy_cef contract — Rust->JS cef.listen delivers a JSON *string* (hence
// JSON.parse); JS->Rust cef.emit serializes only its FIRST argument into one
// global Receive<OzmuxFrame> (single self-describing object, no channel arg, a
// second argument is dropped). New-model host-API bridge: one window[ns] Proxy
// per granted namespace from window.__ozmuxGranted (injected as a separate
// PreloadScript). Binary uses the {__u8} boundary tag (mirrors binary-codec.ts).
(function () {
  var cef = window.cef;
  var nextId = 0;
  var calls = new Map();

  function encodeArg(a) {
    if (a instanceof Uint8Array) {
      var bin = '';
      for (var i = 0; i < a.length; i++) bin += String.fromCharCode(a[i]);
      return { __u8: btoa(bin) };
    }
    return a;
  }

  function decodeValue(v) {
    if (v && typeof v === 'object' && typeof v.__u8 === 'string') {
      var s = atob(v.__u8);
      var out = new Uint8Array(s.length);
      for (var i = 0; i < s.length; i++) out[i] = s.charCodeAt(i);
      return out;
    }
    return v;
  }

  cef.listen('ozmux', function (raw) {
    var frame = typeof raw === 'string' ? JSON.parse(raw) : raw;
    var c = calls.get(frame.reqId);
    if (!c) return;
    calls.delete(frame.reqId);
    if (frame.ok) c.resolve(decodeValue(frame.value));
    else c.reject(new Error(frame.error));
  });

  function hostCall(ns, method, args) {
    var reqId = 'h' + nextId++;
    var encoded = args.map(encodeArg);
    return new Promise(function (resolve, reject) {
      calls.set(reqId, { resolve: resolve, reject: reject });
      cef.emit({ kind: 'host.call', reqId: reqId, ns: ns, method: method, args: encoded });
    });
  }

  var granted = window.__ozmuxGranted || [];
  for (var g = 0; g < granted.length; g++) {
    (function (ns) {
      window[ns] = new Proxy(
        {},
        {
          get: function (_t, method) {
            // NOTE: a Symbol key (e.g. Symbol.toPrimitive, or `then` probing for
            // thenable) must NOT return a callable, or `window[ns]` looks like a
            // Promise and breaks. Only string method names dispatch a host call.
            if (typeof method !== 'string') return undefined;
            return function () {
              return hostCall(ns, method, Array.prototype.slice.call(arguments));
            };
          },
        },
      );
    })(granted[g]);
  }
})();
```

- [ ] **Step 2: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `src/extension_render.rs`. It reuses `spawn_extension_host` (which takes an `extra` bundle) to attach `GrantedNamespaces`:

```rust
    #[test]
    fn new_model_surface_gets_host_bridge_and_granted_list() {
        let mut app = make_test_app();
        app.add_systems(Update, finish_extension_setup);
        let mut caps = std::collections::HashSet::new();
        caps.insert("fs".to_string());
        let (host, ..) = spawn_extension_host(
            &mut app,
            (laid_out_node(Vec2::new(800.0, 600.0)), GrantedNamespaces(caps)),
        );
        app.update();

        let preload = app
            .world()
            .get::<PreloadScripts>(host)
            .expect("new-model surface must carry the host bridge as a PreloadScript");
        assert!(
            preload.0.iter().any(|s| s.contains("kind: 'host.call'")),
            "the host-API bridge JS must be injected for a surface with GrantedNamespaces"
        );
        assert!(
            preload.0.iter().any(|s| s.starts_with("window.__ozmuxGranted=") && s.contains("\"fs\"")),
            "the granted-namespace list must be injected before the bridge"
        );
        assert!(
            !preload.0.iter().any(|s| s == OZMUX_EXTENSION_JS),
            "legacy ozmux.js must NOT be injected for a new-model surface"
        );
    }
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p ozmux-gui --lib -- --test-threads=1 new_model_surface_gets_host_bridge`
Expected: FAIL — `finish_extension_setup` still injects `OZMUX_EXTENSION_JS` unconditionally; `HOST_BRIDGE_JS` not defined.

- [ ] **Step 4: Wire the injection**

In `src/extension_render.rs`, add the embedded bridge constant next to `OZMUX_EXTENSION_JS` (private — used only here):

```rust
const HOST_BRIDGE_JS: &str = include_str!("extension_render/host_bridge.js");
```

Add a `GrantedNamespaces` query to `finish_extension_setup` (immutable params, append to the signature):

```rust
    surfaces: Query<
        (Entity, &ComputedNode),
        (
            With<ExtensionSurfaceMarker>,
            Without<WebviewSource>,
            Without<WebviewMountUnresolved>,
        ),
    >,
    granted: Query<&GrantedNamespaces>,
```

Replace the `PreloadScripts::from([ctx_js, OZMUX_EXTENSION_JS.to_string()])` insertion with a branch selecting the bridge by whether the surface carries `GrantedNamespaces` (new-model) or not (legacy):

```rust
        let ctx_js = context_preload_js(workspace, pane, surface, name);
        let preload = match granted.get(surface) {
            Ok(g) => {
                let list: Vec<&String> = g.0.iter().collect();
                let granted_js = format!(
                    "window.__ozmuxGranted={};",
                    serde_json::to_string(&list).unwrap_or_else(|_| "[]".to_string())
                );
                PreloadScripts::from([ctx_js, granted_js, HOST_BRIDGE_JS.to_string()])
            }
            Err(_) => PreloadScripts::from([ctx_js, OZMUX_EXTENSION_JS.to_string()]),
        };
        commands.entity(surface).insert((
            WebviewSource::new(url),
            WebviewSize(logical),
            preload,
            MaterialNode(materials.add(WebviewUiMaterial::default())),
        ));
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p ozmux-gui --lib -- --test-threads=1 new_model_surface_gets_host_bridge`
Expected: PASS. Confirm the legacy injection test still passes (no `GrantedNamespaces` → legacy branch): `cargo test -p ozmux-gui --lib -- --test-threads=1 attaches_webview_pointed_at_memo_to_extension_host`.

- [ ] **Step 6: Lint + format + commit**

```bash
cargo clippy -p ozmux-gui --all-targets && cargo fmt
git add src/extension_render/host_bridge.js src/extension_render.rs
git commit -m "feat(extension_render): inject host-API Proxy bridge for new-model surfaces"
```

---

## Task 7: Full-workspace verification

**Files:** none (verification only)

- [ ] **Step 1: Host crate + host runtime tests**

Run: `cargo test -p ozmux_extension_host && pnpm -C host test`
Expected: all PASS.

- [ ] **Step 2: GUI crate tests (serialized)**

Run: `cargo test -p ozmux-gui --lib -- --test-threads=1`
Expected: PASS for all `extension_render` / `extension_manager` tests (the known IME failure + teardown SIGSEGV are pre-existing and unrelated; if they appear, confirm they are the documented ones and not introduced here).

- [ ] **Step 3: Build + lint + format gates**

Run: `cargo build && cargo clippy --workspace --all-targets && cargo fmt --check && pnpm -C host check-types`
Expected: clean.

- [ ] **Step 4: Confirm the bundle is current**

Run: `pnpm -C host build && git status --porcelain assets/host.mjs`
Expected: no diff (Task 2 already rebuilt it). If a diff appears, `git add assets/host.mjs` and amend the relevant commit.

- [ ] **Step 5: Final commit (only if Step 4 produced a diff)**

```bash
git add assets/host.mjs
git commit -m "chore(host): rebuild host.mjs bundle"
```

---

## Notes for the executor

- **Not in this step (Step 5/6):** the legacy `window.ozmux.call/subscribe` path, the handlers bridge, the command shim, and the control plane stay untouched; `@memo` keeps working via `ozmux.js`. A real end-to-end test (a live `node` host + a real `extensions/memo` calling `window.fs.read`) lands in Step 6 with the memo migration. Step 4's new bridge is exercised by the unit/integration tests above (capability gate, RPC round-trip, reply routing) plus the host vitest suite.
- **Deferred (recorded, not built here):** bundling `host_bridge.js` from a shared TS module via esbuild (to remove the hand-written/`binary-codec.ts` duplication); `zod` argument validation after the capability gate; MessagePack once the bevy_cef channel carries bytes (spec §5).
- **Single-channel invariant (do not regress):** never add a second `JsEmitEventPlugin::<…>` — the raw IPC receiver is shared and `try_recv`-consumed. The host-call path is a SECOND OBSERVER on the one `Receive<OzmuxFrame>` event; both observers branch on `kind`.
