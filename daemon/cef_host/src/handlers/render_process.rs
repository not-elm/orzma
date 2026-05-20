//! Render-process handler — reads the `extra_info` `DictionaryValue` passed by
//! the browser process in `browser_host_create_browser_sync` and caches it
//! per-browser inside the renderer.
//!
//! Task 5 only populates the cache; binding install (`window.ozmux.context`)
//! follows in Task 6 via `on_context_created`. Storage lives in a
//! `thread_local!` because every CEF callback in the render process runs on
//! the single renderer main thread.

use cef::rc::Rc as _;
use cef::{
    Browser, CefString, DictionaryValue, ImplBrowser, ImplDictionaryValue,
    ImplRenderProcessHandler, RenderProcessHandler, WrapRenderProcessHandler,
    wrap_render_process_handler,
};
use std::cell::RefCell;
use std::collections::HashMap;

/// Per-browser context captured from `extra_info` in `on_browser_created`.
#[derive(Clone, Debug)]
#[expect(dead_code, reason = "fields consumed by Task 6 on_context_created binding install")]
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
            STATES.with(|cell| {
                cell.borrow_mut().remove(&id);
            });
        }
    }
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
