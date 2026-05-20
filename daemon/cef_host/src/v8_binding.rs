//! Conversion helpers between `BrowserExtraContext` and CEF's
//! `DictionaryValue`. The wire side lives in
//! `ozmux_browser_cef_protocol::wire`; this module only handles the CEF DOM
//! representation passed via `extra_info` in
//! `browser_host_create_browser_sync`.
//!
//! Task 5 will add render-process binding install logic here.

use cef::{CefString, DictionaryValue, ImplDictionaryValue, dictionary_value_create};
use ozmux_browser_cef_protocol::wire::{BrowserExtraContext, BrowserRole};

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

fn set_str(dict: &DictionaryValue, key: &str, value: &str) {
    dict.set_string(Some(&CefString::from(key)), Some(&CefString::from(value)));
}
