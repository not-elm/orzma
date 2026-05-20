//! Minimal CefLifeSpanHandler — logs browser lifecycle events and maintains
//! the per-`OzmuxClient` browser_id → activity_id map used by
//! `on_process_message_received` to route render-side bridge messages.

use crate::handlers::client::ClientBrowserMap;
use cef::rc::Rc as _;
use cef::{
    Browser, ImplBrowser, ImplLifeSpanHandler, LifeSpanHandler, WrapLifeSpanHandler,
    wrap_life_span_handler,
};
use ozmux_browser_cef_protocol::types::ActivityId;
use ozmux_browser_cef_protocol::wire::BrowserRole;
use std::sync::Arc;

wrap_life_span_handler! {
    pub struct OzmuxLifeSpanHandler {
        aid: ActivityId,
        role: BrowserRole,
        browser_map: Arc<ClientBrowserMap>,
    }

    impl LifeSpanHandler {
        fn on_after_created(&self, browser: Option<&mut Browser>) {
            tracing::info!(aid = %self.aid.0, "OnAfterCreated");
            if let Some(b) = browser {
                self.browser_map
                    .insert(b.identifier(), self.aid.clone(), self.role);
            }
        }

        fn on_before_close(&self, browser: Option<&mut Browser>) {
            tracing::info!(aid = %self.aid.0, "OnBeforeClose");
            if let Some(b) = browser {
                self.browser_map.remove(b.identifier());
            }
        }
    }
}
