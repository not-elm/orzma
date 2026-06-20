//! In-process webview feature: CEF render wiring and the window.ozma Tier 1
//! back-channel (render), OSC mount/unmount of webviews (osc), and
//! webviews rendered into the terminal text flow (mount). Aggregated
//! behind OzmuxWebviewPlugin.

pub(crate) mod mount;
pub(crate) mod osc;
pub(crate) mod render;

use bevy::prelude::*;
use mount::WebviewPlugin;
use osc::OscPlugin;
use render::RenderPlugin;

/// Bevy plugin aggregating the in-process webview sub-plugins.
pub struct OzmuxWebviewPlugin;

impl Plugin for OzmuxWebviewPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((RenderPlugin, OscPlugin, WebviewPlugin));
    }
}
