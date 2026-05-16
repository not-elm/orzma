//! CEF cookie installation helper: seeds the global `CefCookieManager` from
//! a list of `CefCookieDto` entries received via `BrowserCreate`.
//!
//! The callback-based `set_cookie` API fires on the CEF IO thread. A shared
//! `Arc<AtomicUsize>` pending counter tracks in-flight calls; when it reaches
//! zero the `on_done` closure fires. The caller (pool.rs) wraps `on_done` as
//! a `post_command::post` call back to the UI thread so `CreateBrowserSync`
//! only runs after all cookies have been committed.
//!
//! Phase B Task B12.

use cef::rc::Rc as _;
use cef::{
    Cookie, CookieSameSite, ImplCookieManager, ImplSetCookieCallback, SetCookieCallback,
    WrapSetCookieCallback, cookie_manager_get_global_manager, wrap_set_cookie_callback,
};
use ozmux_browser_cef_protocol::wire::{CefCookieDto, SameSite};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

// NOTE: wrap_set_cookie_callback! must appear at module scope (not inside a
// function) because it emits a struct definition. We define one reusable type
// here and pass per-cookie state through the Arc fields.
wrap_set_cookie_callback! {
    struct PendingCookieCallback {
        pending: Arc<AtomicUsize>,
        on_done: Arc<Mutex<Option<Box<dyn FnOnce() + Send + 'static>>>>,
    }

    impl SetCookieCallback {
        fn on_complete(&self, success: ::std::os::raw::c_int) {
            if success == 0 {
                tracing::warn!("set_cookie callback reported failure");
            }
            let remaining = self.pending.fetch_sub(1, Ordering::AcqRel);
            if remaining == 1 {
                if let Some(f) = self.on_done.lock().expect("on_done poisoned").take() {
                    f();
                }
            }
        }
    }
}

/// Installs `cookies` into the global CEF cookie store, then calls `on_done`.
///
/// When `cookies` is empty, `on_done` is called synchronously before this
/// function returns. When cookies are non-empty, each entry is submitted via
/// `CefCookieManager::set_cookie`; `on_done` fires once all callbacks have
/// completed. Must be called from the CEF UI thread.
///
/// The empty-list fast path is synchronous and safe to call from any context.
pub fn install_cookies(cookies: Vec<CefCookieDto>, on_done: impl FnOnce() + Send + 'static) {
    if cookies.is_empty() {
        on_done();
        return;
    }

    let Some(mgr) = cookie_manager_get_global_manager(None) else {
        tracing::warn!("CookieManager unavailable; proceeding without cookies");
        on_done();
        return;
    };

    let total = cookies.len();
    let pending = Arc::new(AtomicUsize::new(total));
    let on_done_slot: Arc<Mutex<Option<Box<dyn FnOnce() + Send + 'static>>>> =
        Arc::new(Mutex::new(Some(Box::new(on_done))));

    for dto in cookies {
        let url = cef::CefString::from(dto.url.as_str());
        let cookie = build_cef_cookie(&dto);
        let mut cb = PendingCookieCallback::new(Arc::clone(&pending), Arc::clone(&on_done_slot));

        let ret = mgr.set_cookie(Some(&url), Some(&cookie), Some(&mut cb));
        if ret == 0 {
            tracing::warn!(url = %dto.url, "set_cookie returned 0 (failed to enqueue)");
            // NOTE: Decrement the counter ourselves so the overall pending count
            // stays consistent and on_done fires correctly even if some cookies fail.
            let remaining = pending.fetch_sub(1, Ordering::AcqRel);
            if remaining == 1 {
                if let Some(f) = on_done_slot.lock().expect("on_done poisoned").take() {
                    f();
                }
            }
        }
    }
}

fn build_cef_cookie(dto: &CefCookieDto) -> Cookie {
    let same_site = match dto.same_site {
        SameSite::Strict => CookieSameSite::STRICT_MODE,
        SameSite::Lax => CookieSameSite::LAX_MODE,
        SameSite::None => CookieSameSite::NO_RESTRICTION,
        SameSite::Unspecified => CookieSameSite::UNSPECIFIED,
    };

    Cookie {
        name: cef::CefString::from(dto.name.as_str()),
        value: cef::CefString::from(dto.value.as_str()),
        domain: cef::CefString::from(dto.domain.as_str()),
        path: cef::CefString::from(dto.path.as_str()),
        secure: dto.secure as i32,
        httponly: dto.http_only as i32,
        same_site,
        // TODO: Plan 3 — wire real expiry via cef::Basetime when a reliable
        // Windows FILETIME → Basetime conversion helper is available.
        has_expires: 0,
        ..Cookie::default()
    }
}
