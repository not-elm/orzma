//! Standalone terminal webview layer: CEF render wiring, the `window.orzma`
//! Tier 1 back-channel, OSC mount/unmount of webviews anchored to terminal
//! cells, and the control socket that mints Tier 1 handles. Decoupled from any
//! multiplexer; the host maps its surfaces onto `OrzmaTerminal` entities and
//! drives `KeyboardFocused`.

mod control_plane;
mod webview;

use bevy::prelude::*;
use control_plane::ControlPlanePlugin;
pub use control_plane::{ControlPlaneHandle, NormalizedChord, TokenRegistry};
use orzma_webview_host::WebviewAssetRegistry;
use webview::mount::WebviewPlugin;
pub use webview::mount::{
    ForwardKeys, Webview, WebviewHit, focused_webview_of, webview_hit_at, webview_local_dip,
};
pub use webview::osc::NonInteractive;
use webview::osc::OscPlugin;
use webview::render::RenderPlugin;
pub use webview::render::cef_plugin;

/// Bevy plugin: the in-process webview subsystem — CEF render wiring + the
/// `window.orzma` back-channel, OSC mount/unmount, and the control socket.
///
/// The host supplies the `WebviewAssetRegistry` shared with the `orzma://`
/// scheme handler (built via [`cef_plugin`]).
pub struct OrzmaWebviewPlugin {
    /// The registry shared with the `orzma://` scheme handler.
    pub orzma_assets: WebviewAssetRegistry,
}

impl Plugin for OrzmaWebviewPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            ControlPlanePlugin::new(self.orzma_assets.clone()),
            RenderPlugin,
            OscPlugin,
            WebviewPlugin,
        ));
    }
}
