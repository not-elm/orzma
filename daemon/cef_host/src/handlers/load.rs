//! CefLoadHandler — OnLoadingStateChange.
//!
//! Updates `can_back` / `can_forward` in the shared `NavInner` cache and emits
//! `HostEvent::NavStateChanged` so the daemon side can publish the consolidated
//! nav state to subscribers.

use crate::handlers::display::NavInner;
use cef::rc::Rc as _;
use cef::{Browser, ImplLoadHandler, LoadHandler, WrapLoadHandler, wrap_load_handler};
use std::os::raw::c_int;
use std::sync::Arc;

wrap_load_handler! {
    pub struct OzmuxLoadHandler {
        inner: Arc<NavInner>,
    }

    impl LoadHandler {
        fn on_loading_state_change(
            &self,
            _browser: Option<&mut Browser>,
            _is_loading: c_int,
            can_go_back: c_int,
            can_go_forward: c_int,
        ) {
            *self.inner.can_back.lock().expect("NavInner.can_back poisoned") = can_go_back != 0;
            *self.inner.can_forward.lock().expect("NavInner.can_forward poisoned") = can_go_forward != 0;
            self.inner.emit();
        }
    }
}
