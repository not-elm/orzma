//! Standalone terminal webview layer: CEF render wiring, the `window.ozma`
//! Tier 1 back-channel, OSC mount/unmount of webviews anchored to terminal
//! cells, and the control socket that mints Tier 1 handles. Decoupled from any
//! multiplexer; the host maps its surfaces onto `OzmaTerminal` entities and
//! drives `KeyboardFocused`.

mod control_plane;
mod webview;

use bevy::prelude::*;
use control_plane::ControlPlanePlugin;
pub use control_plane::{ControlPlaneHandle, NormalizedChord, TokenRegistry};
use ozmux_webview_host::WebviewAssetRegistry;
use webview::mount::WebviewPlugin;
pub use webview::mount::{
    ForwardKeys, Webview, WebviewHit, focused_webview_of, webview_hit_at, webview_local_dip,
};
use webview::osc::OscPlugin;
pub use webview::osc::{NonInteractive, OscWebviewGate};
use webview::render::RenderPlugin;
pub use webview::render::cef_plugin;

/// Bevy plugin: the in-process webview subsystem — CEF render wiring + the
/// `window.ozma` back-channel, OSC mount/unmount, and the control socket.
///
/// The host supplies the OSC gate's initial state and the `WebviewAssetRegistry`
/// shared with the `ozma-dyn://` scheme handler (built via [`cef_plugin`]).
pub struct OzmaWebviewPlugin {
    /// Initial state of the OSC-webview gate.
    pub osc_enabled: bool,
    /// The registry shared with the `ozma-dyn://` scheme handler.
    pub dyn_assets: WebviewAssetRegistry,
}

impl Plugin for OzmaWebviewPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            ControlPlanePlugin::new(self.dyn_assets.clone()),
            RenderPlugin,
            OscPlugin {
                osc_enabled: self.osc_enabled,
            },
            WebviewPlugin,
        ));
    }
}
