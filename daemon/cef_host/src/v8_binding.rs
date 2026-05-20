//! V8 binding helpers used by the render-process handler.
//!
//! Two concerns live here:
//! 1. `context_to_dict` — serializes `BrowserExtraContext` into the cef-rs
//!    `DictionaryValue` passed to `browser_host_create_browser_sync` so the
//!    renderer can read its activity context back from `extra_info`.
//! 2. `install_window_ozmux` — installs the `window.ozmux` API surface inside
//!    a V8 context: a read-only `context` object plus throwing-stub
//!    `call`/`subscribe` functions (Task 7 replaces the stubs with the real
//!    process-message-backed implementation).

use cef::rc::Rc as _;
use cef::sys::cef_v8_propertyattribute_t;
use cef::{
    CefString, DictionaryValue, ImplDictionaryValue, ImplV8Context, ImplV8Handler, ImplV8Value,
    V8Context, V8Handler, V8Propertyattribute, V8Value, WrapV8Handler, dictionary_value_create,
    v8_value_create_function, v8_value_create_null, v8_value_create_object,
    v8_value_create_string, wrap_v8_handler,
};
use ozmux_browser_cef_protocol::wire::{BrowserExtraContext, BrowserRole};

use crate::handlers::render_process::RenderState;

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
    let Some(ozmux) = v8_value_create_object(None, None) else { return };
    let Some(context_obj) = build_context_object(state) else { return };

    set_readonly(&ozmux, "context", context_obj);

    let call_msg = "ozmux.call not yet implemented".to_string();
    let mut call_handler: V8Handler = ThrowingV8Handler::new(call_msg);
    let call_name = CefString::from("call");
    if let Some(call_fn) = v8_value_create_function(Some(&call_name), Some(&mut call_handler)) {
        set_readonly(&ozmux, "call", call_fn);
    }

    let subscribe_msg = "ozmux.subscribe not yet implemented".to_string();
    let mut subscribe_handler: V8Handler = ThrowingV8Handler::new(subscribe_msg);
    let subscribe_name = CefString::from("subscribe");
    if let Some(subscribe_fn) =
        v8_value_create_function(Some(&subscribe_name), Some(&mut subscribe_handler))
    {
        set_readonly(&ozmux, "subscribe", subscribe_fn);
    }

    set_readonly(&global, "ozmux", ozmux);
}

wrap_v8_handler! {
    pub(crate) struct ThrowingV8Handler {
        msg: String,
    }

    impl V8Handler {
        fn execute(
            &self,
            _name: Option<&CefString>,
            _object: Option<&mut V8Value>,
            _arguments: Option<&[Option<V8Value>]>,
            _retval: Option<&mut Option<V8Value>>,
            exception: Option<&mut CefString>,
        ) -> ::std::os::raw::c_int {
            if let Some(exception) = exception {
                *exception = CefString::from(self.msg.as_str());
            }
            1
        }
    }
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
