//! Minimal CefLifeSpanHandler — logs browser lifecycle events.

use cef::rc::Rc as _;
use cef::{
    Browser, ImplLifeSpanHandler, LifeSpanHandler, WrapLifeSpanHandler, wrap_life_span_handler,
};
use ozmux_browser_cef_protocol::types::ActivityId;

wrap_life_span_handler! {
    pub struct OzmuxLifeSpanHandler {
        aid: ActivityId,
    }

    impl LifeSpanHandler {
        fn on_after_created(&self, _browser: Option<&mut Browser>) {
            tracing::info!(aid = %self.aid.0, "OnAfterCreated");
        }

        fn on_before_close(&self, _browser: Option<&mut Browser>) {
            tracing::info!(aid = %self.aid.0, "OnBeforeClose");
        }
    }
}
