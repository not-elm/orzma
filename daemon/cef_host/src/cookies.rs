//! CEF cookie installation helper: seeds a per-activity `RequestContext`'s
//! `CefCookieManager` from a list of `CefCookieDto` entries received via
//! `BrowserCreate`.
//!
//! The callback-based `set_cookie` completion callback fires on the CEF UI
//! thread. A shared `Arc<AtomicUsize>` pending counter tracks in-flight calls;
//! when it reaches zero the `on_done` closure fires. The caller (pool.rs)
//! wraps `on_done` as a `post_command::post` call back to the UI thread so
//! `CreateBrowserSync` only runs after all cookies have been committed.

use cef::rc::Rc as _;
use cef::{
    Cookie, CookieSameSite, ImplCookieManager, ImplRequestContext, ImplSetCookieCallback,
    RequestContext, SetCookieCallback, WrapSetCookieCallback, wrap_set_cookie_callback,
};
use ozmux_browser_cef_protocol::wire::{CefCookieDto, SameSite};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

/// One-shot completion closure, shared across the per-cookie callbacks and
/// taken by whichever caller observes the pending counter reach zero.
type OnDone = Arc<Mutex<Option<Box<dyn FnOnce() + Send + 'static>>>>;

/// Decrements `pending`; fires `on_done` exactly once when it hits zero.
fn settle(pending: &AtomicUsize, on_done: &OnDone) {
    if pending.fetch_sub(1, Ordering::AcqRel) == 1
        && let Some(f) = on_done.lock().expect("on_done poisoned").take()
    {
        f();
    }
}

// NOTE: wrap_set_cookie_callback! must appear at module scope (not inside a
// function) because it emits a struct definition. We define one reusable type
// here and pass per-cookie state through the Arc fields. `cookie_id` carries
// per-call identity (name/domain/url) into the async callback so a rejection
// can be attributed to the offending cookie — CEF discards Chromium's
// `CookieInclusionStatus`, so the boolean is all we get from the callback.
wrap_set_cookie_callback! {
    struct PendingCookieCallback {
        pending: Arc<AtomicUsize>,
        on_done: OnDone,
        cookie_id: Arc<CookieId>,
    }

    impl SetCookieCallback {
        fn on_complete(&self, success: ::std::os::raw::c_int) {
            if success == 0 {
                tracing::warn!(
                    name = %self.cookie_id.name,
                    domain = %self.cookie_id.domain,
                    url = %self.cookie_id.url,
                    "set_cookie callback reported failure"
                );
            }
            settle(&self.pending, &self.on_done);
        }
    }
}

/// Per-cookie identity carried through `set_cookie`'s async callback for
/// diagnostic logging.
struct CookieId {
    name: String,
    domain: String,
    url: String,
}

/// Installs `cookies` into `request_context`'s cookie manager, then calls
/// `on_done`.
///
/// When `cookies` is empty, `on_done` is called synchronously before this
/// function returns. When cookies are non-empty, each entry is submitted via
/// `CefCookieManager::set_cookie`; `on_done` fires once all callbacks have
/// completed. Must be called from the CEF UI thread.
///
/// The empty-list fast path is synchronous and safe to call from any context.
pub fn install_cookies(
    cookies: Vec<CefCookieDto>,
    request_context: &RequestContext,
    on_done: impl FnOnce() + Send + 'static,
) {
    if cookies.is_empty() {
        on_done();
        return;
    }

    // NOTE: the ready-callback path (`cookie_manager(Some(&mut cb))` + waiting
    // for the CompletionCallback before seeding) is deliberately NOT used here.
    // We pass `None` and seed immediately, relying on CEF internally queueing
    // `set_cookie` calls against the context's storage initialization. This is
    // acceptable because named-profile cache directories are `create_dir_all`'d
    // before the context is created (pool.rs), and CEF queues `set_cookie`
    // against storage init in practice. Risk: a freshly-created context's very
    // first `set_cookie` batch could in principle race storage init; if cookie
    // loss is ever observed here, switch to the `wrap_completion_callback!`
    // ready-callback approach.
    let Some(mgr) = request_context.cookie_manager(None) else {
        tracing::warn!("CookieManager unavailable; proceeding without cookies");
        on_done();
        return;
    };

    let total = cookies.len();
    let pending = Arc::new(AtomicUsize::new(total));
    let on_done_slot: OnDone = Arc::new(Mutex::new(Some(Box::new(on_done))));

    for dto in cookies {
        let url = cef::CefString::from(dto.url.as_str());
        let cookie = build_cef_cookie(&dto);
        let cookie_id = Arc::new(CookieId {
            name: dto.name.clone(),
            domain: dto.domain.clone(),
            url: dto.url.clone(),
        });
        let mut cb = PendingCookieCallback::new(
            Arc::clone(&pending),
            Arc::clone(&on_done_slot),
            Arc::clone(&cookie_id),
        );

        let ret = mgr.set_cookie(Some(&url), Some(&cookie), Some(&mut cb));
        if ret == 0 {
            tracing::warn!(
                name = %cookie_id.name,
                domain = %cookie_id.domain,
                url = %cookie_id.url,
                "set_cookie returned 0 (failed to enqueue)"
            );
            // NOTE: the callback will never fire for this cookie, so settle its
            // slot here to keep the pending count consistent.
            settle(&pending, &on_done_slot);
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
        // TODO: wire real expiry via cef::Basetime once a reliable Windows
        // FILETIME → Basetime conversion helper exists; cookies are
        // session-only until then.
        has_expires: 0,
        ..Cookie::default()
    }
}
