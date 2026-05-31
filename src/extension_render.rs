//! CEF integration for extension activities: the `ozmux-ext://` asset scheme and
//! the `window.ozmux` JS extension, plus the webview spawn-once system that
//! attaches a `bevy_cef` webview to each Extension Activity host. (Bridge = Task 9.)

use crate::system_set::OzmuxSystems;
use crate::ui::ExtensionActivityMarker;
use bevy::prelude::*;
use bevy_cef::prelude::*;
use ozmux_extension_host::host::ExtensionEndpoints;
use ozmux_extension_host::scheme::custom_scheme;

/// The `ozmux-ext://` URL the memo extension webview is pointed at. The host
/// segment (`memo`) matches the extension name registered as a custom scheme
/// in `cef_plugin`; frames for other extension names 404.
const MEMO_WEBVIEW_URL: &str = "ozmux-ext://memo/index.html";

/// The shared asset endpoint the `ozmux-ext://` scheme reads (set on memo-ready
/// in Task 9). Empty until then, so the scheme returns 503.
#[derive(Resource, Clone, Default)]
pub struct AssetEndpoint(pub ExtensionEndpoints);

/// JS defining `window.ozmux` over `cef.emit` / `cef.listen`, injected as a
/// global CEF extension. Mirrors `sdk/typescript/src/cef/ozmux-bridge.ts`.
pub const OZMUX_EXTENSION_JS: &str = include_str!("extension_render/ozmux.js");

/// Builds the `CefPlugin` with the `ozmux-ext://` scheme + `window.ozmux`
/// extension, bound to the given asset endpoint handle.
pub fn cef_plugin(endpoint: &AssetEndpoint) -> CefPlugin {
    CefPlugin {
        // NOTE: "memo" is the extension-NAME segment matched inside
        // ozmux-ext://<name>/…, not the URL scheme name — the scheme is always
        // "ozmux-ext" (scheme::SCHEME_NAME). Frames for other extension names 404.
        custom_schemes: vec![custom_scheme("memo", endpoint.0.clone())],
        extensions: CefExtensions::new().add("ozmux", OZMUX_EXTENSION_JS),
        ..Default::default()
    }
}

/// Wires the spawn-once system that attaches a `bevy_cef` webview to each
/// Extension Activity host.
///
/// Resize is intentionally NOT handled here: `bevy_cef`'s `UiWebviewPlugin`
/// (pulled in by `CefPlugin`) already runs `update_webview_ui_size` in
/// `PostUpdate` after `UiSystems::Layout`, syncing each UI webview's
/// `WebviewSize` from its `ComputedNode`. Adding a second writer would thrash
/// the same component. The terminal path needs its own resize only because it
/// derives grid `cols`/`rows` from font metrics, which `bevy_cef` knows nothing
/// about.
pub struct OzmuxExtensionRenderPlugin;

impl Plugin for OzmuxExtensionRenderPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            finish_extension_setup.in_set(OzmuxSystems::SetupActivity),
        );
    }
}

/// Attaches a `bevy_cef` webview to each freshly-spawned Extension Activity
/// host: a `WebviewSource` pointed at the memo extension plus a
/// `MaterialNode<WebviewUiMaterial>`. Runs every Update tick but only targets
/// hosts that lack `WebviewSource`, so the per-entity insertion happens
/// exactly once.
///
/// The host already carries a full-size `Node` (`width`/`height: 100%`) from
/// `build_activity_host_children`, so the webview fills its pane; `bevy_cef`'s
/// `update_webview_ui_size` keeps `WebviewSize` in step with the node's
/// `ComputedNode` extents on every layout pass.
fn finish_extension_setup(
    mut commands: Commands,
    mut materials: ResMut<Assets<WebviewUiMaterial>>,
    hosts: Query<Entity, (With<ExtensionActivityMarker>, Without<WebviewSource>)>,
) {
    for host in hosts.iter() {
        commands.entity(host).insert((
            WebviewSource::new(MEMO_WEBVIEW_URL),
            MaterialNode(materials.add(WebviewUiMaterial::default())),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::asset::AssetPlugin;
    use bevy::image::ImagePlugin;

    fn make_test_app() -> App {
        // NOTE: `bevy_cef`'s `UiWebviewPlugin` registers `WebviewUiMaterial`
        // through `UiMaterialPlugin`, which pulls in the full render stack. For
        // these headless tests we only need `Assets<WebviewUiMaterial>` to exist
        // so the system's `ResMut<Assets<...>>` parameter resolves. The material
        // is a plain `Asset` (no render-app init required), so `init_asset`
        // suffices — mirrors `ui::terminal`'s `TerminalUiMaterial` test setup.
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(AssetPlugin::default())
            .add_plugins(ImagePlugin::default())
            .init_asset::<WebviewUiMaterial>();
        app
    }

    #[test]
    fn skips_entities_without_extension_marker() {
        let mut app = make_test_app();
        app.add_systems(Update, finish_extension_setup);
        let host = app.world_mut().spawn_empty().id();
        app.update();
        assert!(
            app.world().get::<WebviewSource>(host).is_none(),
            "entity without ExtensionActivityMarker must not receive a WebviewSource"
        );
    }

    #[test]
    fn attaches_webview_pointed_at_memo_to_extension_host() {
        let mut app = make_test_app();
        app.add_systems(Update, finish_extension_setup);
        let host = app.world_mut().spawn(ExtensionActivityMarker).id();
        app.update();

        let source = app
            .world()
            .get::<WebviewSource>(host)
            .expect("extension host must receive a WebviewSource");
        match source {
            WebviewSource::Url(url) => assert_eq!(url, MEMO_WEBVIEW_URL),
            other => panic!("expected a Url source, got {other:?}"),
        }
        assert!(
            app.world()
                .get::<MaterialNode<WebviewUiMaterial>>(host)
                .is_some(),
            "extension host must receive a WebviewUiMaterial MaterialNode"
        );
    }

    #[test]
    fn webview_inserted_exactly_once() {
        let mut app = make_test_app();
        app.add_systems(Update, finish_extension_setup);
        let host = app.world_mut().spawn(ExtensionActivityMarker).id();
        app.update();
        let first = app
            .world()
            .get::<MaterialNode<WebviewUiMaterial>>(host)
            .map(|m| m.id());
        app.update();
        let second = app
            .world()
            .get::<MaterialNode<WebviewUiMaterial>>(host)
            .map(|m| m.id());
        assert_eq!(
            first, second,
            "the second tick must not re-insert (and so not replace) the webview material"
        );
    }
}
