//! Terminal webview layer: CEF render wiring, the `window.orzma` Tier 1
//! back-channel, APC mount and unmount of webviews anchored to terminal cells,
//! and the control socket that mints Tier 1 handles.

mod control_plane;
#[cfg(test)]
mod test_support;
mod webview;

use bevy::prelude::*;
use bevy_orzma_webview_host::WebviewAssetRegistry;
use control_plane::ControlPlanePlugin;
pub use control_plane::{ControlPlaneHandle, HandleId, NormalizedChord, TokenRegistry};
use std::task::Waker;
use webview::apc::ApcPlugin;
pub use webview::apc::{ClickFocusDisabled, NonInteractive};
use webview::mount::WebviewPlugin;
pub use webview::mount::{
    ForwardKeys, Webview, WebviewHit, focused_webview_of, webview_hit_at, webview_local_dip,
};
use webview::paint::PaintPlugin;
use webview::render::RenderPlugin;
pub use webview::render::cef_plugin;

/// The in-process webview subsystem: CEF render wiring, the `window.orzma`
/// back-channel, APC mount and unmount, and the control socket.
pub struct OrzmaWebviewPlugin {
    orzma_assets: WebviewAssetRegistry,
    waker: Waker,
}

impl OrzmaWebviewPlugin {
    /// Builds the plugin sharing `orzma_assets` with the `orzma://` scheme
    /// handler built by [`cef_plugin`]. The control-socket listener wakes the
    /// app through `waker` after it queues work for the app.
    pub fn new(orzma_assets: WebviewAssetRegistry, waker: Waker) -> Self {
        Self {
            orzma_assets,
            waker,
        }
    }
}

impl Plugin for OrzmaWebviewPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            ControlPlanePlugin::new(self.orzma_assets.clone(), self.waker.clone()),
            RenderPlugin,
            ApcPlugin,
            WebviewPlugin,
            PaintPlugin,
        ));
    }
}
