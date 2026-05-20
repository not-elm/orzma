//! Render-process handler — reads the `extra_info` `DictionaryValue` passed by
//! the browser process in `browser_host_create_browser_sync` and caches it
//! per-browser inside the renderer, then installs `window.ozmux` in the V8
//! context of qualifying frames (main frame, role==extension, ozmux-ext://
//! origin, matching `v8_context.is_same`).
//!
//! Storage lives in a `thread_local!` because every CEF callback in the render
//! process runs on the single renderer main thread.

use cef::rc::Rc as _;
use cef::{
    Browser, CefString, DictionaryValue, Frame, ImplBrowser, ImplDictionaryValue, ImplFrame,
    ImplListValue, ImplProcessMessage, ImplRenderProcessHandler, ImplV8Context, ProcessId,
    ProcessMessage, RenderProcessHandler, V8Context, WrapRenderProcessHandler,
    wrap_render_process_handler,
};
use std::cell::RefCell;
use std::collections::HashMap;

use crate::process_message::{MSG_CALL_RESPONSE, MSG_SUB_EVENT};

/// Per-browser context captured from `extra_info` in `on_browser_created`.
#[derive(Clone, Debug)]
pub(crate) struct RenderState {
    pub(crate) role: String,
    pub(crate) session_id: Option<String>,
    pub(crate) window_id: String,
    pub(crate) pane_id: String,
    pub(crate) activity_id: String,
    pub(crate) extension_name: Option<String>,
}

thread_local! {
    /// Render-thread cache keyed by `Browser::identifier()`.
    ///
    /// Populated in `on_browser_created`, released in `on_browser_destroyed`.
    pub(crate) static STATES: RefCell<HashMap<i32, RenderState>> =
        RefCell::new(HashMap::new());
}

wrap_render_process_handler! {
    pub struct OzmuxRenderProcessHandler;

    impl RenderProcessHandler {
        fn on_browser_created(
            &self,
            browser: Option<&mut Browser>,
            extra_info: Option<&mut DictionaryValue>,
        ) {
            let Some(browser) = browser else { return };
            // DevTools / spare renderers come through with no extra_info — skip
            // silently rather than logging on every spare-process spawn.
            let Some(dict) = extra_info else { return };

            let Some(role) = read_string(dict, "role") else { return };
            let Some(window_id) = read_string(dict, "window_id") else { return };
            let Some(pane_id) = read_string(dict, "pane_id") else { return };
            let Some(activity_id) = read_string(dict, "activity_id") else { return };
            let session_id = read_string(dict, "session_id");
            let extension_name = read_string(dict, "extension_name");

            let id = browser.identifier();
            let state = RenderState {
                role,
                session_id,
                window_id,
                pane_id,
                activity_id,
                extension_name,
            };
            STATES.with(|cell| {
                cell.borrow_mut().insert(id, state);
            });
        }

        fn on_browser_destroyed(&self, browser: Option<&mut Browser>) {
            let Some(browser) = browser else { return };
            let id = browser.identifier();
            // Reject any in-flight V8 call() Promises or subscribe()
            // async-iterators bound to this browser. Must happen before
            // STATES is cleared so consumers still have a chance to see
            // a concrete error rather than a Promise that never resolves.
            crate::v8_binding::clear_browser(id);
            STATES.with(|cell| {
                cell.borrow_mut().remove(&id);
            });
        }

        fn on_context_created(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            context: Option<&mut V8Context>,
        ) {
            let (Some(browser), Some(frame), Some(ctx)) = (browser, frame, context) else {
                return;
            };
            if !is_extension_main_frame(browser, frame, ctx) {
                return;
            }
            // NOTE: enter() must be paired with exit() — install_window_ozmux
            // mutates V8 globals and CEF requires the context be entered for
            // any V8 value creation in this thread.
            if ctx.enter() == 0 {
                return;
            }
            let state = STATES.with(|cell| cell.borrow().get(&browser.identifier()).cloned());
            if let Some(state) = state {
                crate::v8_binding::install_window_ozmux(ctx, &state);
            }
            ctx.exit();
        }

        fn on_process_message_received(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            source_process: ProcessId,
            message: Option<&mut ProcessMessage>,
        ) -> ::std::os::raw::c_int {
            // Only browser-process responses are routed here; render→render
            // (would be a bug) and unknown sources are silently dropped.
            if source_process != ProcessId::BROWSER {
                return 0;
            }
            let (Some(frame), Some(message)) = (frame, message) else {
                return 0;
            };
            let name = CefString::from(&message.name()).to_string();
            let Some(args) = message.argument_list() else {
                return 0;
            };
            if args.size() < 1 {
                return 0;
            }
            let payload = CefString::from(&args.string(0)).to_string();
            // V8 calls (resolve_promise / reject_promise) require the context
            // be entered; re-enter from the frame.
            let ctx = match frame.v8_context() {
                Some(c) => c,
                None => return 0,
            };
            if ctx.enter() == 0 {
                return 0;
            }
            match name.as_str() {
                MSG_CALL_RESPONSE => crate::v8_binding::deliver_call_response(&payload),
                MSG_SUB_EVENT => crate::v8_binding::deliver_sub_event(&payload),
                _ => {}
            }
            ctx.exit();
            1
        }
    }
}

fn is_extension_main_frame(browser: &mut Browser, frame: &mut Frame, ctx: &mut V8Context) -> bool {
    if frame.is_main() == 0 {
        return false;
    }
    let mut frame_ctx = frame.v8_context();
    let same = match frame_ctx.as_mut() {
        Some(fc) => fc.is_same(Some(ctx)) != 0,
        None => false,
    };
    if !same {
        return false;
    }
    let url = CefString::from(&frame.url()).to_string();
    if !url.starts_with("ozmux-ext://") {
        return false;
    }
    let id = browser.identifier();
    STATES.with(|cell| {
        cell.borrow()
            .get(&id)
            .map(|state| state.role == "extension")
            .unwrap_or(false)
    })
}

fn read_string(dict: &mut DictionaryValue, key: &str) -> Option<String> {
    let cef_key = CefString::from(key);
    if dict.has_key(Some(&cef_key)) == 0 {
        return None;
    }
    let userfree = dict.string(Some(&cef_key));
    let value = CefString::from(&userfree).to_string();
    if value.is_empty() { None } else { Some(value) }
}
