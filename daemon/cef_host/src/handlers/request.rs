//! CefRequestHandler — defense-in-depth navigation policy.
//!
//! Cancels any navigation from a Browser-role browser to the
//! `ozmux-ext://` scheme. The V8 binding installer in `render_process.rs`
//! already gates `window.ozmux` on `role == "extension"`, but blocking the
//! navigation itself prevents a Browser Activity from even loading
//! extension HTML in the first place — which keeps the V8 install check
//! the only place that needs to reason about role for bridge access.

use crate::handlers::client::ClientBrowserMap;
use cef::rc::Rc as _;
use cef::{
    Browser, CefString, Frame, ImplBrowser, ImplRequest, ImplRequestHandler, Request,
    RequestHandler, WrapRequestHandler, wrap_request_handler,
};
use ozmux_browser_cef_protocol::wire::BrowserRole;
use std::os::raw::c_int;
use std::sync::Arc;

wrap_request_handler! {
    pub struct OzmuxRequestHandler {
        browser_map: Arc<ClientBrowserMap>,
    }

    impl RequestHandler {
        fn on_before_browse(
            &self,
            browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            request: Option<&mut Request>,
            _user_gesture: c_int,
            _is_redirect: c_int,
        ) -> c_int {
            let (Some(browser), Some(request)) = (browser, request) else {
                return 0;
            };
            let url = CefString::from(&request.url()).to_string();
            if !url.starts_with("ozmux-ext://") {
                return 0;
            }
            let role = self.browser_map.role(browser.identifier());
            if matches!(role, Some(BrowserRole::Browser)) {
                tracing::warn!(
                    %url,
                    "Browser Activity attempted to navigate to ozmux-ext:// — blocked",
                );
                return 1;
            }
            0
        }
    }
}
