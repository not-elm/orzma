//! Shared preload-script builder for ozmux-managed webviews: the
//! `window.__ozmuxContext` globals, the `window.__ozmuxGranted` capability
//! grant, and the `window.<ns>.<method>` host-API bridge JS.

use crate::osc_webview::GrantedNamespaces;
use bevy::prelude::Entity;
use bevy_cef::prelude::PreloadScripts;

/// Builds the `ozmux-ext://<name>/<entry>` webview URL for an extension surface,
/// where `entry` is the client's HTML path relative to the extension dir.
pub(crate) fn webview_url(extension_name: &str, entry: &str) -> String {
    format!("ozmux-ext://{extension_name}/{entry}")
}

/// JS defining the new-model `window.<ns>.<method>` host-API bridge over
/// `cef.emit` / `cef.listen`, injected (with `window.__ozmuxGranted`) per
/// webview as a `PreloadScripts` entry.
pub(super) const HOST_BRIDGE_JS: &str = include_str!("host_bridge.js");

/// Builds the full preload script set for an ozmux-managed webview:
/// context globals, the capability grant, and the host-API bridge.
pub(crate) fn build_preload(
    workspace: Entity,
    pane: Entity,
    surface: Entity,
    extension_name: &str,
    granted: &GrantedNamespaces,
) -> PreloadScripts {
    let ctx_js = context_preload_js(workspace, pane, surface, extension_name);
    let granted_json =
        serde_json::to_string(&granted.0).expect("namespace set serializes infallibly");
    let granted_js = format!("window.__ozmuxGranted={granted_json};");
    // NOTE: `window.<ns>` MUST be a PreloadScript, not a global CefExtension.
    // host_bridge.js calls cef.listen() at top level; a global extension runs
    // that during V8 context creation, where there is no entered V8 context, so
    // the native cef.listen handler's v8_context_get_current_context() crashes
    // the render process. PreloadScripts are eval'd at on_context_created inside
    // an entered context (and their exceptions are caught, not fatal), so
    // cef.listen registers correctly there.
    PreloadScripts::from([ctx_js, granted_js, HOST_BRIDGE_JS.to_string()])
}

/// Builds the per-webview context PreloadScript assigning `window.__ozmuxContext`.
///
/// NOTE: PreloadScripts are joined with `;` and eval'd as one unit, so this MUST
/// be a complete statement; a syntax error here would break the bridge eval too.
fn context_preload_js(
    workspace: Entity,
    pane: Entity,
    surface: Entity,
    extension_name: &str,
) -> String {
    let workspace_id = workspace.to_bits().to_string();
    // NOTE: the JS keys "sessionId"/"windowId" keep their legacy names on purpose — a
    // browser-side wire contract the SDK surface client reads; renaming them breaks extensions.
    format!(
        "window.__ozmuxContext={{sessionId:{s:?},windowId:{s:?},paneId:{p:?},surfaceId:{a:?},role:\"extension\",extensionName:{n:?}}};",
        s = workspace_id,
        p = pane.to_bits().to_string(),
        a = surface.to_bits().to_string(),
        n = extension_name,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::app::App;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::prelude::MinimalPlugins;
    use ozmux_multiplexer::{MultiplexerCommands, MultiplexerPlugin};

    #[test]
    fn context_preload_js_assigns_window_context_with_workspace_bits_as_window_id() {
        let world = &mut App::new();
        world
            .add_plugins(MinimalPlugins)
            .add_plugins(MultiplexerPlugin);
        let (workspace, pane, surface) = world
            .world_mut()
            .run_system_once(|mut mux: MultiplexerCommands| {
                let o = mux.create_workspace(Some("t".into()));
                (o.workspace, o.pane, o.surface)
            })
            .unwrap();
        world.world_mut().flush();

        let js = context_preload_js(workspace, pane, surface, "memo");
        let s = workspace.to_bits().to_string();
        assert!(js.starts_with("window.__ozmuxContext="));
        assert!(js.ends_with("};"));
        assert!(js.contains(&format!("sessionId:\"{s}\"")));
        assert!(
            js.contains(&format!("windowId:\"{s}\"")),
            "windowId must equal sessionId per the design"
        );
        assert!(js.contains(&format!("paneId:\"{}\"", pane.to_bits())));
        assert!(js.contains(&format!("surfaceId:\"{}\"", surface.to_bits())));
        assert!(js.contains("role:\"extension\""));
        assert!(js.contains("extensionName:\"memo\""));
    }

    #[test]
    fn build_preload_orders_context_grant_then_bridge() {
        let world = &mut App::new();
        world
            .add_plugins(MinimalPlugins)
            .add_plugins(MultiplexerPlugin);
        let (workspace, pane, surface) = world
            .world_mut()
            .run_system_once(|mut mux: MultiplexerCommands| {
                let o = mux.create_workspace(Some("t".into()));
                (o.workspace, o.pane, o.surface)
            })
            .unwrap();
        world.world_mut().flush();

        let mut caps = std::collections::HashSet::new();
        caps.insert("fs".to_string());
        let preload = build_preload(workspace, pane, surface, "memo", &GrantedNamespaces(caps));

        assert_eq!(preload.0.len(), 3);
        assert!(preload.0[0].starts_with("window.__ozmuxContext="));
        assert_eq!(preload.0[1], "window.__ozmuxGranted=[\"fs\"];");
        assert_eq!(preload.0[2], HOST_BRIDGE_JS);
    }

    #[test]
    fn build_preload_serializes_an_empty_grant_as_empty_array() {
        let world = &mut App::new();
        world
            .add_plugins(MinimalPlugins)
            .add_plugins(MultiplexerPlugin);
        let (workspace, pane, surface) = world
            .world_mut()
            .run_system_once(|mut mux: MultiplexerCommands| {
                let o = mux.create_workspace(Some("t".into()));
                (o.workspace, o.pane, o.surface)
            })
            .unwrap();
        world.world_mut().flush();

        let preload = build_preload(
            workspace,
            pane,
            surface,
            "memo",
            &GrantedNamespaces::default(),
        );
        assert_eq!(preload.0[1], "window.__ozmuxGranted=[];");
    }
}
