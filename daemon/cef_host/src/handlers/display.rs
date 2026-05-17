//! CefDisplayHandler — OnTitleChange / OnAddressChange.
//!
//! Shares mutable cached state (title, url, can_back, can_forward) with the
//! load handler via `Arc<NavInner>`; both handlers update the cache and emit
//! `HostEvent::NavStateChanged` so the daemon side can publish the consolidated
//! nav state to subscribers.

use cef::rc::Rc as _;
use cef::{
    Browser, CefString, CursorInfo, CursorType, DisplayHandler, Frame, ImplDisplayHandler,
    WrapDisplayHandler, wrap_display_handler,
};
use cef_dll_sys::cef_cursor_type_t;
use ozmux_browser_cef_protocol::types::ActivityId;
use ozmux_browser_cef_protocol::wire::{CursorKind, HostEvent};
use std::os::raw::c_int;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

/// Maps a CEF `cef_cursor_type_t` to the semantic [`CursorKind`] the frontend
/// renders. Custom cursor images and rarely-seen kinds fall back to `Default`.
fn cursor_kind_from(type_: &cef_cursor_type_t) -> CursorKind {
    match type_ {
        cef_cursor_type_t::CT_HAND => CursorKind::Pointer,
        cef_cursor_type_t::CT_IBEAM | cef_cursor_type_t::CT_VERTICALTEXT => CursorKind::Text,
        cef_cursor_type_t::CT_CROSS => CursorKind::Crosshair,
        cef_cursor_type_t::CT_WAIT => CursorKind::Wait,
        cef_cursor_type_t::CT_PROGRESS => CursorKind::Progress,
        cef_cursor_type_t::CT_HELP => CursorKind::Help,
        cef_cursor_type_t::CT_MOVE => CursorKind::Move,
        cef_cursor_type_t::CT_NOTALLOWED | cef_cursor_type_t::CT_NODROP => CursorKind::NotAllowed,
        cef_cursor_type_t::CT_GRAB => CursorKind::Grab,
        cef_cursor_type_t::CT_GRABBING => CursorKind::Grabbing,
        cef_cursor_type_t::CT_EASTRESIZE
        | cef_cursor_type_t::CT_WESTRESIZE
        | cef_cursor_type_t::CT_EASTWESTRESIZE
        | cef_cursor_type_t::CT_COLUMNRESIZE => CursorKind::ColResize,
        cef_cursor_type_t::CT_NORTHRESIZE
        | cef_cursor_type_t::CT_SOUTHRESIZE
        | cef_cursor_type_t::CT_NORTHSOUTHRESIZE
        | cef_cursor_type_t::CT_ROWRESIZE => CursorKind::RowResize,
        cef_cursor_type_t::CT_NORTHEASTRESIZE
        | cef_cursor_type_t::CT_SOUTHWESTRESIZE
        | cef_cursor_type_t::CT_NORTHEASTSOUTHWESTRESIZE => CursorKind::NeswResize,
        cef_cursor_type_t::CT_NORTHWESTRESIZE
        | cef_cursor_type_t::CT_SOUTHEASTRESIZE
        | cef_cursor_type_t::CT_NORTHWESTSOUTHEASTRESIZE => CursorKind::NwseResize,
        cef_cursor_type_t::CT_ZOOMIN => CursorKind::ZoomIn,
        cef_cursor_type_t::CT_ZOOMOUT => CursorKind::ZoomOut,
        _ => CursorKind::Default,
    }
}

/// Shared navigation state cache, updated by both the display and load handlers.
///
/// Every field is individually `Mutex`-guarded because the display and load
/// handlers may run concurrently on the CEF UI thread (they are distinct vtable
/// slots). Using one coarser lock would be safe too, but fine-grained guards
/// avoid head-of-line blocking between independent updates.
pub struct NavInner {
    /// The activity this nav state belongs to.
    pub aid: ActivityId,
    /// Unbounded sender into the cef_host → daemon event channel.
    pub event_tx: mpsc::UnboundedSender<HostEvent>,
    /// Current page URL.
    pub url: Mutex<String>,
    /// Current page title.
    pub title: Mutex<String>,
    /// `true` if back navigation is available.
    pub can_back: Mutex<bool>,
    /// `true` if forward navigation is available.
    pub can_forward: Mutex<bool>,
}

impl NavInner {
    /// Creates a new `NavInner` with empty defaults.
    pub fn new(aid: ActivityId, event_tx: mpsc::UnboundedSender<HostEvent>) -> Arc<Self> {
        Arc::new(Self {
            aid,
            event_tx,
            url: Mutex::new(String::new()),
            title: Mutex::new(String::new()),
            can_back: Mutex::new(false),
            can_forward: Mutex::new(false),
        })
    }

    /// Reads all cached fields and emits a `HostEvent::NavStateChanged`.
    pub fn emit(&self) {
        let url = self.url.lock().expect("NavInner.url poisoned").clone();
        let title = self.title.lock().expect("NavInner.title poisoned").clone();
        let can_back = *self.can_back.lock().expect("NavInner.can_back poisoned");
        let can_forward = *self
            .can_forward
            .lock()
            .expect("NavInner.can_forward poisoned");
        let _ = self.event_tx.send(HostEvent::NavStateChanged {
            aid: self.aid.clone(),
            url,
            title,
            can_back,
            can_forward,
        });
    }
}

wrap_display_handler! {
    pub struct OzmuxDisplayHandler {
        inner: Arc<NavInner>,
    }

    impl DisplayHandler {
        fn on_title_change(&self, _browser: Option<&mut Browser>, title: Option<&CefString>) {
            let new_title = title.map(|t| t.to_string()).unwrap_or_default();
            *self.inner.title.lock().expect("NavInner.title poisoned") = new_title;
            self.inner.emit();
        }

        fn on_address_change(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            url: Option<&CefString>,
        ) {
            let new_url = url.map(|u| u.to_string()).unwrap_or_default();
            *self.inner.url.lock().expect("NavInner.url poisoned") = new_url;
            self.inner.emit();
        }

        fn on_cursor_change(
            &self,
            _browser: Option<&mut Browser>,
            _cursor: *mut u8,
            type_: CursorType,
            _custom_cursor_info: Option<&CursorInfo>,
        ) -> c_int {
            let cursor = cursor_kind_from(type_.as_ref());
            let _ = self.inner.event_tx.send(HostEvent::CursorChanged {
                aid: self.inner.aid.clone(),
                cursor,
            });
            // NOTE: windowless rendering — there is no OS cursor to set here;
            // 0 lets CEF apply its default handling.
            0
        }
    }
}
