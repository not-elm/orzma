//! CEF ClientHandler aggregator — exposes RenderHandler, LifeSpanHandler,
//! DisplayHandler, LoadHandler, ContextMenuHandler, and RequestHandler to
//! a `browser_host_create_browser_sync` call.
//!
//! Also fields render-process messages (`ozmux.call.request`,
//! `ozmux.sub.open`, `ozmux.sub.cancel`) for the V8 ↔ extension bridge by
//! caching a per-browser `(ActivityId, BrowserRole)` planted in
//! `on_after_created` (see `lifespan.rs`) and forwarding the payload to
//! the [`crate::extension_bridge::ExtensionBridge`]. The role is also
//! consulted by [`crate::handlers::request::OzmuxRequestHandler`] to
//! deny Browser → `ozmux-ext://` navigations.

#![allow(
    clippy::too_many_arguments,
    reason = "wrap_client! synthesizes `OzmuxClient::new` with one arg per handler; each handler \
              is required by the embedded browser and bundling them would just move the same \
              fields one call deeper"
)]

use crate::extension_bridge::ExtensionBridge;
use crate::process_message::{MSG_CALL_REQUEST, MSG_SUB_CANCEL, MSG_SUB_OPEN};
use cef::rc::Rc as _;
use cef::{
    Browser, CefString, Client, ContextMenuHandler, DisplayHandler, Frame, ImplBrowser, ImplClient,
    ImplListValue, ImplProcessMessage, LifeSpanHandler, LoadHandler, ProcessId, ProcessMessage,
    RenderHandler, RequestHandler, WrapClient, wrap_client,
};
use ozmux_browser_cef_protocol::types::ActivityId;
use ozmux_browser_cef_protocol::wire::BrowserRole;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Per-browser context used to route process messages back onto the
/// extension UDS and to gate navigation policy. Populated in
/// `OzmuxLifeSpanHandler::on_after_created` (which has the `Browser`
/// handle), cleared in `on_before_close`.
#[derive(Default)]
pub struct ClientBrowserMap {
    inner: Mutex<HashMap<i32, BrowserEntry>>,
}

/// Per-browser-id record kept by [`ClientBrowserMap`]: the owning activity
/// (for bridge routing) and the activity's role (for navigation policy).
#[derive(Clone)]
pub(crate) struct BrowserEntry {
    pub aid: ActivityId,
    pub role: BrowserRole,
}

impl ClientBrowserMap {
    /// Records the activity that owns `browser_id` together with its role.
    /// Called once per browser lifecycle from the LifeSpanHandler on the
    /// CEF UI thread.
    pub fn insert(&self, browser_id: i32, aid: ActivityId, role: BrowserRole) {
        self.inner
            .lock()
            .expect("client browser map poisoned")
            .insert(browser_id, BrowserEntry { aid, role });
    }

    /// Forgets the mapping; called when a browser is being destroyed.
    pub fn remove(&self, browser_id: i32) {
        self.inner
            .lock()
            .expect("client browser map poisoned")
            .remove(&browser_id);
    }

    /// Returns the role of the browser identified by `browser_id`, if known.
    /// Used by `OzmuxRequestHandler::on_before_browse` to enforce that only
    /// Extension-role browsers may navigate to `ozmux-ext://` URLs.
    pub(crate) fn role(&self, browser_id: i32) -> Option<BrowserRole> {
        self.inner
            .lock()
            .expect("client browser map poisoned")
            .get(&browser_id)
            .map(|e| e.role)
    }

    fn get_aid(&self, browser_id: i32) -> Option<ActivityId> {
        self.inner
            .lock()
            .expect("client browser map poisoned")
            .get(&browser_id)
            .map(|e| e.aid.clone())
    }
}

wrap_client! {
    pub struct OzmuxClient {
        render: RenderHandler,
        life_span: LifeSpanHandler,
        display: DisplayHandler,
        load: LoadHandler,
        context_menu: ContextMenuHandler,
        request: RequestHandler,
        bridge: Option<ExtensionBridge>,
        browser_map: Arc<ClientBrowserMap>,
    }

    impl Client {
        fn render_handler(&self) -> Option<RenderHandler> {
            Some(self.render.clone())
        }

        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(self.life_span.clone())
        }

        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(self.display.clone())
        }

        fn load_handler(&self) -> Option<LoadHandler> {
            Some(self.load.clone())
        }

        fn context_menu_handler(&self) -> Option<ContextMenuHandler> {
            Some(self.context_menu.clone())
        }

        fn request_handler(&self) -> Option<RequestHandler> {
            Some(self.request.clone())
        }

        fn on_process_message_received(
            &self,
            browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            source_process: ProcessId,
            message: Option<&mut ProcessMessage>,
        ) -> ::std::os::raw::c_int {
            // Only renderer-sourced messages are routable; anything else is a
            // bug or a fuzz attempt and is silently dropped.
            if source_process != ProcessId::RENDERER {
                return 0;
            }
            let (Some(browser), Some(message)) = (browser, message) else {
                return 0;
            };
            let Some(bridge) = self.bridge.as_ref() else {
                tracing::warn!("on_process_message_received: bridge not installed");
                return 0;
            };
            let name = CefString::from(&message.name()).to_string();
            let Some(args) = message.argument_list() else {
                tracing::warn!(%name, "process message has no argument list");
                return 0;
            };
            if args.size() < 1 {
                tracing::warn!(%name, "process message has no payload arg");
                return 0;
            }
            let payload_json = CefString::from(&args.string(0)).to_string();
            let aid = match self.browser_map.get_aid(browser.identifier()) {
                Some(a) => a,
                None => {
                    tracing::warn!(
                        browser_id = browser.identifier(),
                        %name,
                        "process message arrived for unmapped browser; dropping",
                    );
                    return 0;
                }
            };
            match name.as_str() {
                MSG_CALL_REQUEST | MSG_SUB_OPEN | MSG_SUB_CANCEL => {
                    let frame_json = render_frame_json(name.as_str(), &payload_json);
                    bridge.forward(aid, frame_json);
                    1
                }
                _ => {
                    tracing::warn!(%name, "unknown ozmux process message");
                    0
                }
            }
        }
    }
}

/// Re-stamps the render-side JSON (CallRequest / SubOpen / SubCancel) into
/// the SDK protocol shape (HandlerCallFrame / SubOpenFrame / SubCancelFrame)
/// expected by the extension UDS. The translation is purely a `kind` field
/// rename — the inner id / name / payload fields are already shaped to
/// match.
fn render_frame_json(message_name: &str, payload_json: &str) -> String {
    let kind = match message_name {
        MSG_CALL_REQUEST => "call",
        MSG_SUB_OPEN => "sub.open",
        MSG_SUB_CANCEL => "sub.cancel",
        _ => return payload_json.to_string(),
    };
    // Parse, inject `kind`, re-emit. We re-parse rather than string-patching
    // so a malformed or unexpected payload surfaces as a JSON error in the
    // bridge logs instead of being silently mangled.
    match serde_json::from_str::<serde_json::Value>(payload_json) {
        Ok(mut v) => {
            if let Some(obj) = v.as_object_mut() {
                obj.insert(
                    "kind".to_string(),
                    serde_json::Value::String(kind.to_string()),
                );
            }
            v.to_string()
        }
        Err(e) => {
            tracing::warn!(error = %e, "render_frame_json: payload not JSON");
            payload_json.to_string()
        }
    }
}
