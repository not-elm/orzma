//! Per-activity UDS connection pool to Node extension processes.
//!
//! Translates V8 binding requests (received as CEF process messages in the
//! browser-process `OzmuxClient::on_process_message_received`) into the
//! `{ aid, frame }` JSON-Lines envelope that the Node extension UDS
//! protocol uses, and forwards extension responses back to the originating
//! render process as CEF process messages.
//!
//! Threading model:
//!
//! - `ExtensionBridge` is `Send + Sync` and cheap to `Clone` (it wraps an
//!   `Arc<Mutex<HashMap<…>>>`).
//! - Dispatch from the CEF UI thread (`OzmuxClient::on_process_message_received`)
//!   uses `runtime.spawn(...)` to enqueue an envelope line onto the per-aid
//!   writer channel; the spawned future does not touch any CEF object.
//! - Reads from the UDS happen on a Tokio task; each line is decoded and
//!   handed to a `CefCommand::DispatchExtensionResponse` posted onto the CEF
//!   UI thread via `post_command::post`. The UI thread does the
//!   `Frame::send_process_message(PID::RENDERER, …)` so all CEF refcounted
//!   types stay on their owning thread.

use crate::pool::CefCommand;
use crate::post_command::{self, PoolHandle};
use crate::process_message::{CallResponse, MSG_CALL_RESPONSE, MSG_SUB_EVENT, SubEvent};
use futures_util::{SinkExt, StreamExt};
use ozmux_browser_cef_protocol::types::ActivityId;
use ozmux_browser_cef_protocol::wire::BrowserUnavailableReason;
use ozmux_extension::registry::ExtensionRegistry;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tokio_util::codec::{FramedRead, FramedWrite, LinesCodec};

/// Callback invoked by the bridge when an activity's UDS connection fails
/// (either initial connect or mid-stream EOF/IO error). The callee should
/// surface this to any `/browser/ws` subscriber as a
/// `BrowserUnavailable { aid: Some(_), reason }` event.
pub type UnavailableCallback = Arc<dyn Fn(ActivityId, BrowserUnavailableReason) + Send + Sync>;

/// Outbound message kind carried on the bridge; equivalent to the
/// `CallResponse` / `SubEvent` shape but parameterized so we can reuse the
/// same UDS reader for both call results and subscription events.
#[derive(Debug, Deserialize)]
#[serde(tag = "kind")]
enum HandlerUdsFrameIn {
    #[serde(rename = "result")]
    Result { id: String, payload: Value },
    #[serde(rename = "error")]
    Error {
        id: String,
        code: String,
        message: String,
    },
    #[serde(rename = "sub.data")]
    SubData { id: String, payload: Value },
    #[serde(rename = "sub.complete")]
    SubComplete { id: String },
    #[serde(rename = "sub.error")]
    SubError {
        id: String,
        code: String,
        message: String,
    },
}

#[derive(Debug, Deserialize)]
struct EnvelopeIn {
    frame: Value,
}

/// Routed response after UDS decode: one process message back to the
/// originating render frame. Posted as a `CefCommand::DispatchExtensionResponse`
/// to the CEF UI thread.
///
/// The variant payloads (`CallResponse`, `SubEvent`) are intentionally not
/// re-exported through this type's public API — call sites only need
/// `aid()`, `message_name()`, and `payload_json()` accessors, which expose
/// strings/identifiers rather than the raw types.
#[derive(Debug)]
pub struct BridgeDispatch {
    aid: ActivityId,
    inner: BridgeDispatchInner,
}

#[derive(Debug)]
enum BridgeDispatchInner {
    CallResponse(CallResponse),
    SubEvent(SubEvent),
}

impl BridgeDispatch {
    /// The activity this response should be routed back to.
    pub fn aid(&self) -> &ActivityId {
        &self.aid
    }

    /// The CEF process-message name corresponding to this response. Either
    /// [`MSG_CALL_RESPONSE`] or [`MSG_SUB_EVENT`].
    pub fn message_name(&self) -> &'static str {
        match self.inner {
            BridgeDispatchInner::CallResponse(_) => MSG_CALL_RESPONSE,
            BridgeDispatchInner::SubEvent(_) => MSG_SUB_EVENT,
        }
    }

    /// Serializes the inner payload as a JSON string for the CEF process
    /// message argument list. Returns an empty `{}` on serialization failure
    /// (impossible in practice — both inner types are pure JSON).
    pub fn payload_json(&self) -> String {
        match &self.inner {
            BridgeDispatchInner::CallResponse(p) => {
                serde_json::to_string(p).unwrap_or_else(|_| "{}".to_string())
            }
            BridgeDispatchInner::SubEvent(p) => {
                serde_json::to_string(p).unwrap_or_else(|_| "{}".to_string())
            }
        }
    }
}

/// Bridge from V8/render process messages to extension UDS handlers.
#[derive(Clone)]
pub struct ExtensionBridge {
    inner: Arc<Mutex<HashMap<ActivityId, mpsc::Sender<String>>>>,
    runtime: tokio::runtime::Handle,
    extensions: ExtensionRegistry,
    pool: PoolHandle,
    unavailable_cb: Option<UnavailableCallback>,
}

impl ExtensionBridge {
    /// Creates a fresh bridge. `runtime` is the Tokio runtime handle that
    /// will own the per-connection reader/writer tasks; `extensions` is the
    /// daemon-wide registry consulted to find the owning extension and its
    /// handlers UDS path; `pool` is the CEF UI-thread pool used to post the
    /// `DispatchExtensionResponse` command after a UDS read.
    pub fn new(
        runtime: tokio::runtime::Handle,
        extensions: ExtensionRegistry,
        pool: PoolHandle,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            runtime,
            extensions,
            pool,
            unavailable_cb: None,
        }
    }

    /// Installs the per-activity unavailable callback used to surface UDS
    /// connect/EOF failures to the `/browser/ws` subscribers. Replaces any
    /// previously installed callback; calling this with `Arc::clone` of the
    /// same closure is idempotent in practice.
    pub fn with_unavailable_callback(mut self, cb: UnavailableCallback) -> Self {
        self.unavailable_cb = Some(cb);
        self
    }

    fn notify_unavailable(&self, aid: &ActivityId, reason: BrowserUnavailableReason) {
        if let Some(cb) = &self.unavailable_cb {
            cb(aid.clone(), reason);
        }
    }

    /// Forwards a render-originated frame (already serialized as JSON without
    /// the envelope) to the owning extension. The first call for a given
    /// `aid` opens a fresh UDS connection and spawns its reader; subsequent
    /// calls reuse the existing connection.
    pub fn forward(&self, aid: ActivityId, frame_json: String) {
        let me = self.clone();
        self.runtime.spawn(async move {
            if let Err(e) = me.forward_inner(&aid, frame_json).await {
                tracing::warn!(?aid, error = %e, "extension_bridge: forward failed");
            }
        });
    }

    async fn forward_inner(&self, aid: &ActivityId, frame_json: String) -> Result<(), String> {
        let line = format!(
            r#"{{"aid":{},"frame":{}}}"#,
            serde_json::to_string(&aid.0).map_err(|e| e.to_string())?,
            frame_json
        );
        let tx = self.ensure_connection(aid).await?;
        tx.send(line)
            .await
            .map_err(|_| "channel closed".to_string())
    }

    async fn ensure_connection(&self, aid: &ActivityId) -> Result<mpsc::Sender<String>, String> {
        if let Some(tx) = self
            .inner
            .lock()
            .expect("bridge poisoned")
            .get(aid)
            .cloned()
        {
            return Ok(tx);
        }
        let sock_path = self.lookup_sock_path(aid)?;
        let stream = match UnixStream::connect(&sock_path).await {
            Ok(s) => s,
            Err(e) => {
                self.notify_unavailable(aid, BrowserUnavailableReason::ExtensionDisconnected);
                return Err(format!("UDS connect {}: {}", sock_path.display(), e));
            }
        };
        let (tx, rx) = mpsc::channel::<String>(64);
        {
            let mut g = self.inner.lock().expect("bridge poisoned");
            // NOTE: another caller may have raced us between the initial check
            // and the connect; if so, drop our just-opened stream and reuse
            // theirs. Avoids two concurrent readers on the same UDS.
            if let Some(existing) = g.get(aid).cloned() {
                return Ok(existing);
            }
            g.insert(aid.clone(), tx.clone());
        }
        self.spawn_connection(aid.clone(), stream, rx);
        Ok(tx)
    }

    fn lookup_sock_path(&self, aid: &ActivityId) -> Result<PathBuf, String> {
        let ext_name = self
            .extensions
            .activity_owner_by_str(&aid.0)
            .ok_or_else(|| format!("activity {} has no owning extension", aid.0))?;
        self.extensions
            .handlers_sock_path(&ext_name)
            .ok_or_else(|| format!("extension {} not running (no sock path)", ext_name))
    }

    fn spawn_connection(&self, aid: ActivityId, stream: UnixStream, rx: mpsc::Receiver<String>) {
        let pool = self.pool.clone();
        let inner = self.inner.clone();
        let cb = self.unavailable_cb.clone();
        self.runtime.spawn(async move {
            run_connection(aid.clone(), stream, rx, pool).await;
            inner.lock().expect("bridge poisoned").remove(&aid);
            if let Some(cb) = cb {
                cb(aid.clone(), BrowserUnavailableReason::ExtensionDisconnected);
            }
            tracing::debug!(?aid, "extension_bridge: connection closed");
        });
    }
}

async fn run_connection(
    aid: ActivityId,
    stream: UnixStream,
    mut rx: mpsc::Receiver<String>,
    pool: PoolHandle,
) {
    let (uds_r, uds_w) = stream.into_split();
    let mut framed_w = FramedWrite::new(uds_w, LinesCodec::new());
    let mut framed_r = FramedRead::new(uds_r, LinesCodec::new_with_max_length(1 << 20));

    let writer_aid = aid.clone();
    let writer = tokio::spawn(async move {
        while let Some(line) = rx.recv().await {
            if let Err(e) = framed_w.send(line).await {
                tracing::warn!(?writer_aid, error = %e, "extension_bridge: UDS write failed");
                break;
            }
        }
    });

    while let Some(item) = framed_r.next().await {
        let line = match item {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!(?aid, error = %e, "extension_bridge: UDS read failed");
                break;
            }
        };
        match decode_envelope(&line) {
            Ok(Some(dispatch)) => {
                let dispatch = BridgeDispatch::with_aid(dispatch, aid.clone());
                if let Err(e) =
                    post_command::post(&pool, CefCommand::DispatchExtensionResponse { dispatch })
                {
                    tracing::warn!(?aid, error = %e, "extension_bridge: post to UI thread failed");
                }
            }
            Ok(None) => {
                tracing::warn!(?aid, line = %line, "extension_bridge: dropped unknown frame");
            }
            Err(e) => {
                tracing::warn!(?aid, error = %e, "extension_bridge: envelope decode failed");
            }
        }
    }
    writer.abort();
}

/// Decodes a `{aid, frame}` envelope line into a `BridgeDispatchKind`.
/// Returns `Ok(None)` if the frame is recognised JSON but not a routable
/// response kind (defensive — extension can only emit handler kinds).
fn decode_envelope(line: &str) -> Result<Option<BridgeDispatchInner>, String> {
    let env: EnvelopeIn = serde_json::from_str(line).map_err(|e| e.to_string())?;
    let frame: HandlerUdsFrameIn = serde_json::from_value(env.frame).map_err(|e| e.to_string())?;
    Ok(Some(match frame {
        HandlerUdsFrameIn::Result { id, payload } => {
            BridgeDispatchInner::CallResponse(CallResponse::Result { id, payload })
        }
        HandlerUdsFrameIn::Error { id, code, message } => {
            BridgeDispatchInner::CallResponse(CallResponse::Error { id, code, message })
        }
        HandlerUdsFrameIn::SubData { id, payload } => {
            BridgeDispatchInner::SubEvent(SubEvent::Data { id, payload })
        }
        HandlerUdsFrameIn::SubComplete { id } => {
            BridgeDispatchInner::SubEvent(SubEvent::Complete { id })
        }
        HandlerUdsFrameIn::SubError { id, code, message } => {
            BridgeDispatchInner::SubEvent(SubEvent::Error { id, code, message })
        }
    }))
}

impl BridgeDispatch {
    fn with_aid(inner: BridgeDispatchInner, aid: ActivityId) -> Self {
        Self { aid, inner }
    }
}

#[cfg(test)]
fn build_test_envelope(aid: &str, frame: Value) -> String {
    let v = serde_json::json!({ "aid": aid, "frame": frame });
    serde_json::to_string(&v).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn decode_envelope_handles_result() {
        let line = build_test_envelope(
            "a1",
            json!({ "kind": "result", "id": "c1", "payload": {"hi": "yo"} }),
        );
        let out = decode_envelope(&line).unwrap().unwrap();
        match out {
            BridgeDispatchInner::CallResponse(CallResponse::Result { id, payload }) => {
                assert_eq!(id, "c1");
                assert_eq!(payload, json!({"hi": "yo"}));
            }
            _ => panic!("expected CallResponse::Result"),
        }
    }

    #[test]
    fn decode_envelope_handles_sub_data_complete_error() {
        let cases = vec![
            (
                json!({ "kind": "sub.data", "id": "s1", "payload": {"i": 0} }),
                "data",
            ),
            (json!({ "kind": "sub.complete", "id": "s1" }), "complete"),
            (
                json!({ "kind": "sub.error", "id": "s1", "code": "X", "message": "m" }),
                "error",
            ),
        ];
        for (frame, expect) in cases {
            let line = build_test_envelope("a1", frame);
            let out = decode_envelope(&line).unwrap().unwrap();
            match (out, expect) {
                (BridgeDispatchInner::SubEvent(SubEvent::Data { .. }), "data") => {}
                (BridgeDispatchInner::SubEvent(SubEvent::Complete { .. }), "complete") => {}
                (BridgeDispatchInner::SubEvent(SubEvent::Error { .. }), "error") => {}
                _ => panic!("unexpected SubEvent variant for case {expect}"),
            }
        }
    }

    #[test]
    fn decode_envelope_rejects_garbage() {
        assert!(decode_envelope("not json").is_err());
    }

    // Verify that `run_connection`'s EOF path invokes the configured
    // unavailable callback exactly once. The callback is invoked by the
    // spawned task wrapper in `spawn_connection`, after `run_connection`
    // returns and the entry is removed from the pool.
    #[tokio::test(flavor = "current_thread")]
    async fn run_connection_eof_invokes_unavailable_callback() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::net::UnixListener;

        // Server: accept, drop immediately → reader sees EOF.
        let tmp = tempfile::tempdir().unwrap();
        let sock_path = tmp.path().join("h.sock");
        let listener = UnixListener::bind(&sock_path).unwrap();
        let server = tokio::spawn(async move {
            let _ = listener.accept().await;
            // dropping listener + accepted stream → client side reads EOF
        });

        // Register an extension that owns the activity and points at the sock.
        let registry = ozmux_extension::registry::ExtensionRegistry::default();
        let ext_name = "ext-eof-test";
        registry.register(ext_name, std::path::Path::new("."));
        registry.set_handlers_sock_path(ext_name, &sock_path);
        let aid = ActivityId("a-eof".into());
        // ozmux_multiplexer::ActivityId has a private inner field; construct
        // via deserialization to get a stable string-keyed id.
        let mux_aid: ozmux_multiplexer::ActivityId = serde_json::from_str(r#""a-eof""#).unwrap();
        registry.record_activity_owner(&mux_aid, ext_name);

        // Build a bridge with a counting callback. The bridge needs a
        // PoolHandle to post DispatchExtensionResponse — provide a real
        // one constructed from a BrowserPool stub.
        let (event_tx, _) =
            tokio::sync::mpsc::unbounded_channel::<ozmux_browser_cef_protocol::wire::HostEvent>();
        let frame_pool = std::sync::Arc::new(crate::frame_buffer_pool::FrameBufferPool::new(2));
        let pool = crate::pool::BrowserPool::new(
            event_tx,
            std::env::temp_dir(),
            false,
            0,
            frame_pool,
            registry.clone(),
        );
        let pool_handle = crate::post_command::PoolHandle::new(pool);

        let counter = Arc::new(AtomicUsize::new(0));
        let last_reason = Arc::new(Mutex::new(None::<BrowserUnavailableReason>));
        let counter_cb = Arc::clone(&counter);
        let last_cb = Arc::clone(&last_reason);
        let cb: UnavailableCallback = Arc::new(move |_aid, reason| {
            counter_cb.fetch_add(1, Ordering::SeqCst);
            *last_cb.lock().unwrap() = Some(reason);
        });

        let bridge = ExtensionBridge::new(tokio::runtime::Handle::current(), registry, pool_handle)
            .with_unavailable_callback(cb);

        // Trigger the forward → connect → spawn-reader flow. The server
        // will drop the accepted stream, so the reader sees EOF and the
        // spawn_connection wrapper fires the callback.
        bridge.forward(
            aid.clone(),
            r#"{"kind":"result","id":"c","payload":{}}"#.to_string(),
        );

        // Wait until callback fires (best-effort poll loop).
        for _ in 0..50 {
            if counter.load(Ordering::SeqCst) > 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let _ = server.await;

        assert_eq!(counter.load(Ordering::SeqCst), 1);
        assert!(matches!(
            last_reason.lock().unwrap().clone(),
            Some(BrowserUnavailableReason::ExtensionDisconnected)
        ));
    }
}
