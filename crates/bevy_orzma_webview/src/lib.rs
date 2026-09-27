//! Terminal webview layer: CEF render wiring, the `window.orzma` page
//! bridge, and the webviews the webview host mounts in terminal panes.

pub mod error;
mod webview;

use bevy::prelude::*;
pub use error::{WebviewError, WebviewResult};
use webview::assets::AssetsPlugin;
pub use webview::focus::RequestWebviewFocus;
use webview::focus::WebviewFocusPlugin;
use webview::forward_keys::ForwardKeysPlugin;
pub use webview::forward_keys::{ChordKey, ForwardKeys, NormalizedChord};
use webview::mount::WebviewPlugin;
pub use webview::mount::{
    NonInteractive, Webview, WebviewHit, focused_webview_of, webview_hit_at, webview_local_dip,
};
use webview::paint::PaintPlugin;
use webview::render::RenderPlugin;
pub use webview::render::cef_plugin;
pub use webview::scheme::WebviewAssetRegistry;

/// The in-process webview subsystem: CEF render wiring, the `window.orzma`
/// page bridge, and the webviews the host mounts.
pub struct OrzmaWebviewPlugin {
    orzma_assets: WebviewAssetRegistry,
}

impl OrzmaWebviewPlugin {
    /// Builds the plugin sharing `orzma_assets` with the `orzma://` scheme
    /// handler built by [`cef_plugin`].
    pub fn new(orzma_assets: WebviewAssetRegistry) -> Self {
        Self { orzma_assets }
    }
}

impl Plugin for OrzmaWebviewPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            AssetsPlugin::new(self.orzma_assets.clone()),
            ForwardKeysPlugin,
            WebviewFocusPlugin,
            RenderPlugin,
            WebviewPlugin,
            PaintPlugin,
        ));
    }
}
