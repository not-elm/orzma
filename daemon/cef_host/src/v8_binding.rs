//! V8 binding for `window.ozmux` (`context`, `call`, `subscribe`).
//!
//! Three concerns live here:
//!
//! 1. `context_to_dict` — serializes `BrowserExtraContext` into a CEF
//!    `DictionaryValue` passed via `extra_info` to
//!    `browser_host_create_browser_sync`. The render side reads this back in
//!    `on_browser_created`.
//! 2. `install_window_ozmux` — installs `window.ozmux.__native_call` and
//!    `window.ozmux.__native_subscribe` as native V8 functions backed by
//!    Rust handlers, then runs a small JS wrapper (via `V8Context::eval`)
//!    that re-exposes them as the documented Promise / AsyncIterable surface
//!    (`window.ozmux.call`, `window.ozmux.subscribe`).
//! 3. `RENDER_STATE` — per-render-thread registry of pending Promises and
//!    subscription queues, keyed by id. Populated by
//!    `OzmuxRenderProcessHandler::on_process_message_received` (see
//!    `handlers/render_process.rs`).
//!
//! Threading: the render process is single-threaded, so `thread_local!` is
//! sufficient for the pending-call / pending-subscription registries.

use cef::rc::Rc as _;
use cef::sys::cef_v8_propertyattribute_t;
use cef::{
    CefString, DictionaryValue, ImplBrowser, ImplDictionaryValue, ImplFrame, ImplListValue,
    ImplProcessMessage, ImplV8Context, ImplV8Handler, ImplV8Value, ProcessId, V8Context, V8Handler,
    V8Propertyattribute, V8Value, WrapV8Handler, dictionary_value_create, list_value_create,
    process_message_create, v8_value_create_function, v8_value_create_null, v8_value_create_object,
    v8_value_create_promise, v8_value_create_string, wrap_v8_handler,
};
use ozmux_browser_cef_protocol::wire::{BrowserExtraContext, BrowserRole};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};

use crate::handlers::render_process::RenderState;
use crate::process_message::{
    CallResponse, HandlerOutgoingFrame, MSG_CALL_REQUEST, MSG_SUB_CANCEL, MSG_SUB_OPEN, SubEvent,
};

thread_local! {
    /// Per-render-thread registry. Each entry tracks an in-flight `call()`
    /// or `subscribe()` originated from V8. The render process is single
    /// threaded so this thread-local is the natural home.
    static RENDER_STATE: RefCell<RenderBindingState> =
        RefCell::new(RenderBindingState::default());
}

#[derive(Default)]
struct RenderBindingState {
    pending_calls: HashMap<String, V8Value>,
    pending_subs: HashMap<String, SubChannel>,
    /// Parallel index: which pending call/sub ids belong to which browser.
    /// Used by [`clear_browser`] when a browser is destroyed so we don't
    /// leave dangling Promises that never resolve or async iterators that
    /// never error out. Each id appears in exactly one bucket.
    by_browser: HashMap<i32, HashSet<String>>,
    next_id: u64,
}

struct SubChannel {
    queue: VecDeque<String>,
    done: bool,
    error: Option<String>,
    /// A pending `next()` Promise waiting for the next event. At most one —
    /// the JS wrapper does not call `next()` concurrently because the
    /// AsyncIterable consumer awaits each result.
    waker: Option<V8Value>,
}

impl RenderBindingState {
    fn mint_id(&mut self, prefix: &str) -> String {
        self.next_id += 1;
        format!("{}{}", prefix, self.next_id)
    }

    fn register_pending(&mut self, browser_id: i32, id: String) {
        self.by_browser.entry(browser_id).or_default().insert(id);
    }

    fn forget_pending(&mut self, id: &str) {
        // Cheap: the entry is in at most one browser's bucket, so we walk
        // until found. Browser counts per renderer process are small.
        let mut empty_bucket: Option<i32> = None;
        for (bid, set) in self.by_browser.iter_mut() {
            if set.remove(id) {
                if set.is_empty() {
                    empty_bucket = Some(*bid);
                }
                break;
            }
        }
        if let Some(bid) = empty_bucket {
            self.by_browser.remove(&bid);
        }
    }
}

/// Rejects every in-flight `call()` Promise and errors every active
/// `subscribe()` AsyncIterable that belongs to `browser_id`. Called from
/// `OzmuxRenderProcessHandler::on_browser_destroyed` so consumers see a
/// concrete failure (instead of a Promise that never settles) when the
/// host yanks the browser mid-call.
pub(crate) fn clear_browser(browser_id: i32) {
    let ids = RENDER_STATE.with(|cell| {
        cell.borrow_mut()
            .by_browser
            .remove(&browser_id)
            .unwrap_or_default()
    });
    if ids.is_empty() {
        return;
    }
    let err_msg = "browser destroyed before response";
    RENDER_STATE.with(|cell| {
        let mut state = cell.borrow_mut();
        for id in ids {
            if let Some(promise) = state.pending_calls.remove(&id) {
                promise.reject_promise(Some(&CefString::from(err_msg)));
                continue;
            }
            if let Some(mut ch) = state.pending_subs.remove(&id) {
                ch.error = Some(err_msg.to_string());
                ch.done = true;
                if let Some(waker) = ch.waker.take() {
                    waker.reject_promise(Some(&CefString::from(err_msg)));
                }
            }
        }
    });
}

/// Returns the V8 host browser's identifier for the current V8 context.
/// Used when registering a fresh pending call/subscription so [`clear_browser`]
/// can find and reject it later on `on_browser_destroyed`.
fn current_browser_id() -> Option<i32> {
    let ctx = cef::v8_context_get_current_context()?;
    let frame = ctx.frame()?;
    let browser = frame.browser()?;
    Some(browser.identifier())
}

/// Called by `OzmuxRenderProcessHandler::on_process_message_received` when a
/// `MSG_CALL_RESPONSE` arrives. Looks up the pending call, resolves/rejects
/// its V8 Promise, and removes the entry. Must be called with the V8 context
/// already entered.
pub(crate) fn deliver_call_response(payload_json: &str) {
    let response: CallResponse = match serde_json::from_str(payload_json) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "call response payload not decodable");
            return;
        }
    };
    let (id, outcome) = match response {
        CallResponse::Result { id, payload } => (id, Ok(payload)),
        CallResponse::Error { id, message, .. } => (id, Err(message)),
    };
    let promise = RENDER_STATE.with(|cell| {
        let mut state = cell.borrow_mut();
        let promise = state.pending_calls.remove(&id);
        if promise.is_some() {
            state.forget_pending(&id);
        }
        promise
    });
    let Some(promise) = promise else {
        tracing::warn!(id = %id, "call response for unknown id (dropped)");
        return;
    };
    match outcome {
        Ok(payload) => {
            let payload_str =
                serde_json::to_string(&payload).unwrap_or_else(|_| "null".to_string());
            let Some(mut v) = v8_value_create_string(Some(&CefString::from(payload_str.as_str())))
            else {
                tracing::error!("v8_value_create_string returned None resolving call");
                return;
            };
            promise.resolve_promise(Some(&mut v));
        }
        Err(msg) => {
            promise.reject_promise(Some(&CefString::from(msg.as_str())));
        }
    }
}

/// Called by `OzmuxRenderProcessHandler::on_process_message_received` when a
/// `MSG_SUB_EVENT` arrives. Pushes the event into the per-subscription queue
/// and resolves any waiting `next()` Promise. Must be called with the V8
/// context already entered.
pub(crate) fn deliver_sub_event(payload_json: &str) {
    let event: SubEvent = match serde_json::from_str(payload_json) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "sub event payload not decodable");
            return;
        }
    };
    let id = match &event {
        SubEvent::Data { id, .. } | SubEvent::Complete { id } | SubEvent::Error { id, .. } => {
            id.clone()
        }
    };
    RENDER_STATE.with(|cell| {
        let mut state = cell.borrow_mut();
        let Some(ch) = state.pending_subs.get_mut(&id) else {
            return;
        };
        match event {
            SubEvent::Data { payload, .. } => {
                let s = serde_json::to_string(&payload).unwrap_or_else(|_| "null".to_string());
                ch.queue.push_back(s);
            }
            SubEvent::Complete { .. } => {
                ch.done = true;
            }
            SubEvent::Error { message, .. } => {
                ch.error = Some(message);
                ch.done = true;
            }
        }
        if let Some(waker) = ch.waker.take() {
            drain_one_into_waker(&id, ch, waker);
        }
    });
}

/// Pops one event from `ch` and resolves the supplied `next()` waker
/// promise. Caller must hold the RENDER_STATE borrow and have already
/// extracted the waker (`Option::take`) so this fn does not need to.
fn drain_one_into_waker(_id: &str, ch: &mut SubChannel, waker: V8Value) {
    // sub.error has higher priority than queued data — if both arrive in the
    // same tick we surface the error first so callers see the failure.
    if let Some(err) = ch.error.take() {
        waker.reject_promise(Some(&CefString::from(err.as_str())));
        return;
    }
    if let Some(s) = ch.queue.pop_front() {
        let Some(obj) = build_iter_result(false, Some(s.as_str())) else {
            return;
        };
        let mut obj = obj;
        waker.resolve_promise(Some(&mut obj));
        return;
    }
    if ch.done {
        let Some(obj) = build_iter_result(true, None) else {
            return;
        };
        let mut obj = obj;
        waker.resolve_promise(Some(&mut obj));
    }
}

fn build_iter_result(done: bool, value_json: Option<&str>) -> Option<V8Value> {
    let obj = v8_value_create_object(None, None)?;
    let done_v = cef::v8_value_create_bool(if done { 1 } else { 0 })?;
    set_value(&obj, "done", done_v);
    if let Some(s) = value_json {
        let v = v8_value_create_string(Some(&CefString::from(s)))?;
        set_value(&obj, "value", v);
    } else {
        let v = v8_value_create_null()?;
        set_value(&obj, "value", v);
    }
    Some(obj)
}

/// Serializes a `BrowserExtraContext` into a fresh `DictionaryValue` for
/// `browser_host_create_browser_sync(..., extra_info, ...)`. The render side
/// reads back the same keys in `on_browser_created`.
pub(crate) fn context_to_dict(ctx: &BrowserExtraContext) -> Option<DictionaryValue> {
    let dict = dictionary_value_create()?;
    let role = match ctx.role {
        BrowserRole::Browser => "browser",
        BrowserRole::Extension => "extension",
    };
    set_str(&dict, "role", role);
    if let Some(sid) = ctx.session_id.as_deref() {
        set_str(&dict, "session_id", sid);
    }
    set_str(&dict, "window_id", &ctx.window_id);
    set_str(&dict, "pane_id", &ctx.pane_id);
    set_str(&dict, "activity_id", &ctx.activity_id);
    if let Some(ext) = ctx.extension_name.as_deref() {
        set_str(&dict, "extension_name", ext);
    }
    Some(dict)
}

/// Installs `window.ozmux` (with `context`, `call`, and `subscribe`) into the
/// currently-entered V8 context. Caller must have already invoked
/// `ctx.enter()` and is responsible for the matching `ctx.exit()`.
pub(crate) fn install_window_ozmux(ctx: &mut V8Context, state: &RenderState) {
    let Some(global) = ctx.global() else { return };
    let Some(ozmux) = v8_value_create_object(None, None) else {
        return;
    };
    let Some(context_obj) = build_context_object(state) else {
        return;
    };
    set_readonly(&ozmux, "context", context_obj);

    let call_name = CefString::from("__native_call");
    let mut call_handler: V8Handler = CallHandler::new();
    if let Some(call_fn) = v8_value_create_function(Some(&call_name), Some(&mut call_handler)) {
        set_readonly(&ozmux, "__native_call", call_fn);
    }

    let sub_name = CefString::from("__native_subscribe");
    let mut sub_handler: V8Handler = SubscribeHandler::new();
    if let Some(sub_fn) = v8_value_create_function(Some(&sub_name), Some(&mut sub_handler)) {
        set_readonly(&ozmux, "__native_subscribe", sub_fn);
    }

    set_readonly(&global, "ozmux", ozmux);

    // Install the JS wrapper that adapts the native string-based primitives
    // into the documented `Promise<any>` / `AsyncIterable<Event>` API.
    let url = CefString::from("ozmux-internal://v8-wrapper.js");
    let code = CefString::from(WRAPPER_JS);
    let mut retval: Option<V8Value> = None;
    let mut exception: Option<cef::V8Exception> = None;
    if ctx.eval(
        Some(&code),
        Some(&url),
        0,
        Some(&mut retval),
        Some(&mut exception),
    ) == 0
    {
        tracing::error!("install_window_ozmux: wrapper eval failed");
    }
}

/// JS that runs once per extension-frame context. Wraps the native string
/// primitives with a Promise / AsyncIterable façade matching the SDK
/// contract.
const WRAPPER_JS: &str = r#"
(function() {
  const m = window.ozmux;
  if (!m) return;
  const nativeCall = m.__native_call;
  const nativeSubscribe = m.__native_subscribe;

  Object.defineProperty(m, 'call', {
    configurable: false,
    enumerable: true,
    writable: false,
    value: function call(name, payload) {
      const payloadJson = payload === undefined ? 'null' : JSON.stringify(payload);
      return nativeCall(name, payloadJson).then(function(s) {
        return s == null ? null : JSON.parse(s);
      });
    },
  });

  Object.defineProperty(m, 'subscribe', {
    configurable: false,
    enumerable: true,
    writable: false,
    value: function subscribe(name, params, opts) {
      const paramsJson = params === undefined ? 'null' : JSON.stringify(params);
      const handle = nativeSubscribe(name, paramsJson);
      const signal = opts && opts.signal;
      let cancelled = false;
      function doCancel() {
        if (cancelled) return;
        cancelled = true;
        try { handle.cancel(); } catch (_) {}
      }
      if (signal) {
        if (signal.aborted) doCancel();
        else signal.addEventListener('abort', doCancel, { once: true });
      }
      return {
        [Symbol.asyncIterator]() {
          return {
            next() {
              if (cancelled) return Promise.resolve({ value: undefined, done: true });
              return handle.next().then(function(r) {
                if (r && r.done) return { value: undefined, done: true };
                const v = r && r.value;
                return { value: v == null ? null : JSON.parse(v), done: false };
              });
            },
            return() {
              doCancel();
              return Promise.resolve({ value: undefined, done: true });
            },
          };
        },
      };
    },
  });
})();
"#;

wrap_v8_handler! {
    pub(crate) struct CallHandler;

    impl V8Handler {
        fn execute(
            &self,
            _name: Option<&CefString>,
            _object: Option<&mut V8Value>,
            arguments: Option<&[Option<V8Value>]>,
            retval: Option<&mut Option<V8Value>>,
            exception: Option<&mut CefString>,
        ) -> ::std::os::raw::c_int {
            let args = arguments.unwrap_or(&[]);
            let name_str = match args.first().and_then(|a| a.as_ref()) {
                Some(v) if v.is_string() != 0 => {
                    CefString::from(&v.string_value()).to_string()
                }
                _ => {
                    if let Some(ex) = exception {
                        *ex = CefString::from("ozmux.call: name must be a string");
                    }
                    return 1;
                }
            };
            let payload_json = match args.get(1).and_then(|a| a.as_ref()) {
                Some(v) if v.is_string() != 0 => {
                    CefString::from(&v.string_value()).to_string()
                }
                _ => "null".to_string(),
            };
            let Some(promise) = v8_value_create_promise() else {
                if let Some(ex) = exception {
                    *ex = CefString::from("ozmux.call: failed to create promise");
                }
                return 1;
            };
            let id = RENDER_STATE.with(|cell| cell.borrow_mut().mint_id("c"));
            let browser_id = current_browser_id();
            RENDER_STATE.with(|cell| {
                let mut state = cell.borrow_mut();
                state.pending_calls.insert(id.clone(), promise.clone());
                if let Some(bid) = browser_id {
                    state.register_pending(bid, id.clone());
                }
            });
            if !send_call_request(&id, &name_str, &payload_json) {
                // Failed to send; reject immediately rather than leak the entry.
                RENDER_STATE.with(|cell| {
                    let mut state = cell.borrow_mut();
                    state.pending_calls.remove(&id);
                    state.forget_pending(&id);
                });
                promise.reject_promise(Some(&CefString::from("ozmux.call: send failed")));
            }
            if let Some(rv) = retval {
                *rv = Some(promise);
            }
            1
        }
    }
}

wrap_v8_handler! {
    pub(crate) struct SubscribeHandler;

    impl V8Handler {
        fn execute(
            &self,
            _name: Option<&CefString>,
            _object: Option<&mut V8Value>,
            arguments: Option<&[Option<V8Value>]>,
            retval: Option<&mut Option<V8Value>>,
            exception: Option<&mut CefString>,
        ) -> ::std::os::raw::c_int {
            let args = arguments.unwrap_or(&[]);
            let name_str = match args.first().and_then(|a| a.as_ref()) {
                Some(v) if v.is_string() != 0 => {
                    CefString::from(&v.string_value()).to_string()
                }
                _ => {
                    if let Some(ex) = exception {
                        *ex = CefString::from("ozmux.subscribe: name must be a string");
                    }
                    return 1;
                }
            };
            let params_json = match args.get(1).and_then(|a| a.as_ref()) {
                Some(v) if v.is_string() != 0 => {
                    CefString::from(&v.string_value()).to_string()
                }
                _ => "null".to_string(),
            };
            let id = RENDER_STATE.with(|cell| cell.borrow_mut().mint_id("s"));
            let browser_id = current_browser_id();
            RENDER_STATE.with(|cell| {
                let mut state = cell.borrow_mut();
                state.pending_subs.insert(
                    id.clone(),
                    SubChannel { queue: VecDeque::new(), done: false, error: None, waker: None },
                );
                if let Some(bid) = browser_id {
                    state.register_pending(bid, id.clone());
                }
            });
            if !send_sub_open(&id, &name_str, &params_json) {
                RENDER_STATE.with(|cell| {
                    let mut state = cell.borrow_mut();
                    state.pending_subs.remove(&id);
                    state.forget_pending(&id);
                });
                if let Some(ex) = exception {
                    *ex = CefString::from("ozmux.subscribe: send failed");
                }
                return 1;
            }
            // Build the per-subscription handle: { next, cancel }
            let Some(handle) = v8_value_create_object(None, None) else {
                if let Some(ex) = exception {
                    *ex = CefString::from("ozmux.subscribe: failed to create handle");
                }
                return 1;
            };
            let next_name = CefString::from("next");
            let mut next_handler: V8Handler = SubNextHandler::new(id.clone());
            if let Some(f) = v8_value_create_function(Some(&next_name), Some(&mut next_handler)) {
                set_value(&handle, "next", f);
            }
            let cancel_name = CefString::from("cancel");
            let mut cancel_handler: V8Handler = SubCancelHandler::new(id);
            if let Some(f) = v8_value_create_function(Some(&cancel_name), Some(&mut cancel_handler)) {
                set_value(&handle, "cancel", f);
            }
            if let Some(rv) = retval {
                *rv = Some(handle);
            }
            1
        }
    }
}

wrap_v8_handler! {
    pub(crate) struct SubNextHandler {
        id: String,
    }

    impl V8Handler {
        fn execute(
            &self,
            _name: Option<&CefString>,
            _object: Option<&mut V8Value>,
            _arguments: Option<&[Option<V8Value>]>,
            retval: Option<&mut Option<V8Value>>,
            _exception: Option<&mut CefString>,
        ) -> ::std::os::raw::c_int {
            let Some(promise) = v8_value_create_promise() else { return 1 };
            // Best effort: try to drain a queued event synchronously to
            // avoid a microtask hop. If nothing is available, install a
            // waker for the next sub event.
            RENDER_STATE.with(|cell| {
                let mut state = cell.borrow_mut();
                let Some(ch) = state.pending_subs.get_mut(&self.id) else {
                    // Subscription already cleaned up: resolve as { done:true }.
                    if let Some(obj) = build_iter_result(true, None) {
                        let mut obj = obj;
                        promise.resolve_promise(Some(&mut obj));
                    }
                    return;
                };
                if let Some(err) = ch.error.take() {
                    promise.reject_promise(Some(&CefString::from(err.as_str())));
                    return;
                }
                if let Some(s) = ch.queue.pop_front() {
                    if let Some(obj) = build_iter_result(false, Some(s.as_str())) {
                        let mut obj = obj;
                        promise.resolve_promise(Some(&mut obj));
                    }
                    return;
                }
                if ch.done {
                    if let Some(obj) = build_iter_result(true, None) {
                        let mut obj = obj;
                        promise.resolve_promise(Some(&mut obj));
                    }
                    return;
                }
                ch.waker = Some(promise.clone());
            });
            if let Some(rv) = retval {
                *rv = Some(promise);
            }
            1
        }
    }
}

wrap_v8_handler! {
    pub(crate) struct SubCancelHandler {
        id: String,
    }

    impl V8Handler {
        fn execute(
            &self,
            _name: Option<&CefString>,
            _object: Option<&mut V8Value>,
            _arguments: Option<&[Option<V8Value>]>,
            _retval: Option<&mut Option<V8Value>>,
            _exception: Option<&mut CefString>,
        ) -> ::std::os::raw::c_int {
            let removed = RENDER_STATE.with(|cell| {
                let mut state = cell.borrow_mut();
                let removed = state.pending_subs.remove(&self.id);
                if removed.is_some() {
                    state.forget_pending(&self.id);
                }
                removed
            });
            if removed.is_some() {
                send_sub_cancel(&self.id);
            }
            1
        }
    }
}

fn send_call_request(id: &str, name: &str, payload_json: &str) -> bool {
    let payload = parse_inner_json(payload_json);
    let frame = HandlerOutgoingFrame::Call {
        id: id.to_string(),
        name: name.to_string(),
        payload,
    };
    let Ok(s) = serde_json::to_string(&frame) else {
        tracing::error!("serialize HandlerOutgoingFrame::Call failed");
        return false;
    };
    send_process_message(MSG_CALL_REQUEST, &s)
}

fn send_sub_open(id: &str, name: &str, params_json: &str) -> bool {
    let params = parse_inner_json(params_json);
    let frame = HandlerOutgoingFrame::SubOpen {
        id: id.to_string(),
        name: name.to_string(),
        params,
    };
    let Ok(s) = serde_json::to_string(&frame) else {
        tracing::error!("serialize HandlerOutgoingFrame::SubOpen failed");
        return false;
    };
    send_process_message(MSG_SUB_OPEN, &s)
}

fn send_sub_cancel(id: &str) -> bool {
    let frame = HandlerOutgoingFrame::SubCancel { id: id.to_string() };
    let Ok(s) = serde_json::to_string(&frame) else {
        tracing::error!("serialize HandlerOutgoingFrame::SubCancel failed");
        return false;
    };
    send_process_message(MSG_SUB_CANCEL, &s)
}

/// Parses a JSON literal received from the JS wrapper. JS always
/// pre-serializes user payloads with `JSON.stringify` (or sends the literal
/// `"null"` when `undefined`); a parse failure here means the JS wrapper
/// passed something non-JSON and we silently coerce to `null` rather than
/// dropping the call.
fn parse_inner_json(s: &str) -> serde_json::Value {
    serde_json::from_str(s).unwrap_or(serde_json::Value::Null)
}

/// Sends a CEF process message from the current V8 context's main frame to
/// the browser process. Looks the frame up via
/// `v8_context_get_current_context()` rather than threading a `Browser`
/// through every call site.
fn send_process_message(message_name: &str, payload_json: &str) -> bool {
    let Some(ctx) = cef::v8_context_get_current_context() else {
        tracing::warn!("send_process_message: no current V8 context");
        return false;
    };
    let Some(frame) = ctx.frame() else {
        tracing::warn!("send_process_message: V8 context has no frame");
        return false;
    };
    let cef_name = CefString::from(message_name);
    let Some(mut msg) = process_message_create(Some(&cef_name)) else {
        tracing::error!(name = message_name, "process_message_create returned None");
        return false;
    };
    let Some(args) = msg.argument_list().or_else(list_value_create) else {
        tracing::error!("argument_list returned None");
        return false;
    };
    args.set_string(0, Some(&CefString::from(payload_json)));
    frame.send_process_message(ProcessId::BROWSER, Some(&mut msg));
    true
}

fn build_context_object(state: &RenderState) -> Option<V8Value> {
    let obj = v8_value_create_object(None, None)?;
    set_readonly_str(&obj, "role", &state.role);
    set_readonly_str(&obj, "windowId", &state.window_id);
    set_readonly_str(&obj, "paneId", &state.pane_id);
    set_readonly_str(&obj, "activityId", &state.activity_id);
    match state.session_id.as_deref() {
        Some(sid) => set_readonly_str(&obj, "sessionId", sid),
        None => {
            if let Some(null_value) = v8_value_create_null() {
                set_readonly(&obj, "sessionId", null_value);
            }
        }
    }
    if let Some(ext) = state.extension_name.as_deref() {
        set_readonly_str(&obj, "extensionName", ext);
    }
    Some(obj)
}

fn set_readonly(obj: &V8Value, key: &str, mut value: V8Value) {
    let cef_key = CefString::from(key);
    obj.set_value_bykey(Some(&cef_key), Some(&mut value), readonly_attributes());
}

fn set_value(obj: &V8Value, key: &str, mut value: V8Value) {
    let cef_key = CefString::from(key);
    obj.set_value_bykey(
        Some(&cef_key),
        Some(&mut value),
        V8Propertyattribute::default(),
    );
}

fn set_readonly_str(obj: &V8Value, key: &str, value: &str) {
    let Some(string_value) = v8_value_create_string(Some(&CefString::from(value))) else {
        return;
    };
    set_readonly(obj, key, string_value);
}

fn readonly_attributes() -> V8Propertyattribute {
    let raw = cef_v8_propertyattribute_t::V8_PROPERTY_ATTRIBUTE_READONLY.0
        | cef_v8_propertyattribute_t::V8_PROPERTY_ATTRIBUTE_DONTDELETE.0;
    cef_v8_propertyattribute_t(raw).into()
}

fn set_str(dict: &DictionaryValue, key: &str, value: &str) {
    dict.set_string(Some(&CefString::from(key)), Some(&CefString::from(value)));
}

// Allow tests / future code paths to reach the registry while keeping the
// guts opaque outside.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mint_id_increments() {
        let mut s = RenderBindingState::default();
        let a = s.mint_id("c");
        let b = s.mint_id("c");
        assert_ne!(a, b);
        assert!(a.starts_with('c'));
        assert!(b.starts_with('c'));
    }

    #[test]
    fn forget_pending_removes_from_by_browser() {
        let mut s = RenderBindingState::default();
        s.register_pending(7, "c1".to_string());
        s.register_pending(7, "s1".to_string());
        s.register_pending(8, "c2".to_string());

        s.forget_pending("c1");
        assert!(s.by_browser.get(&7).unwrap().contains("s1"));
        assert!(!s.by_browser.get(&7).unwrap().contains("c1"));

        s.forget_pending("s1");
        assert!(!s.by_browser.contains_key(&7), "empty bucket pruned");

        s.forget_pending("c2");
        assert!(s.by_browser.is_empty());
    }

    #[test]
    fn forget_pending_no_op_for_unknown_id() {
        let mut s = RenderBindingState::default();
        s.register_pending(1, "a".to_string());
        s.forget_pending("zzz");
        assert!(s.by_browser.get(&1).unwrap().contains("a"));
    }

    // NOTE: silence unused warnings for the param of `deliver_call_response`
    // / `deliver_sub_event` when the rest of the module compiles without
    // them being exercised by integration tests. The render-process handler
    // calls these from a real V8 context which we can't construct in unit
    // tests.
    #[test]
    fn delivery_helpers_short_circuit_on_unknown_id() {
        // No panic for unknown id; both helpers no-op silently.
        deliver_call_response(r#"{"kind":"result","id":"zzz","payload":null}"#);
        deliver_sub_event(r#"{"kind":"sub.complete","id":"zzz"}"#);
    }

    /// Ensures we silently ignore malformed JSON instead of panicking.
    #[test]
    fn delivery_helpers_ignore_garbage() {
        deliver_call_response("not json");
        deliver_sub_event("not json");
    }
}
