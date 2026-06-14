//! Shared preload-script builder for ozmux-managed webviews: the
//! `window.__ozmuxContext` globals and the `window.ozmux` Tier 1 back-channel
//! bridge (`ozmux_bridge.js`).

use bevy::prelude::Entity;
use bevy_cef::prelude::PreloadScripts;

/// JS defining the unified `window.ozmux` back-channel bridge (`.call` / `.on`),
/// injected per Tier 1 dynamic webview as a `PreloadScripts` entry. Frozen onto
/// `window` so a page cannot shadow it.
pub(super) const OZMUX_BRIDGE_JS: &str = include_str!("ozmux_bridge.js");

/// Builds the preload for a Tier 1 dynamic webview: context globals + the
/// `window.ozmux` back-channel bridge. No capability grant (the bridge routes to
/// the registering program, not the host).
pub(crate) fn build_dynamic_preload(
    workspace: Entity,
    pane: Entity,
    surface: Entity,
) -> PreloadScripts {
    let ctx_js = context_preload_js_role(workspace, pane, surface, "dynamic");
    PreloadScripts::from([ctx_js, OZMUX_BRIDGE_JS.to_string()])
}

/// Builds the per-webview context PreloadScript assigning `window.__ozmuxContext`
/// with the given `role`.
///
/// NOTE: the JS keys "sessionId"/"windowId" keep their legacy names on purpose — a
/// browser-side wire contract the SDK surface client reads; renaming them breaks the SDK.
fn context_preload_js_role(workspace: Entity, pane: Entity, surface: Entity, role: &str) -> String {
    let workspace_id = workspace.to_bits().to_string();
    format!(
        "window.__ozmuxContext={{sessionId:{s:?},windowId:{s:?},paneId:{p:?},surfaceId:{a:?},role:{r:?}}};",
        s = workspace_id,
        p = pane.to_bits().to_string(),
        a = surface.to_bits().to_string(),
        r = role,
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
    fn context_preload_js_role_assigns_window_context_with_workspace_bits_as_window_id() {
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

        let js = context_preload_js_role(workspace, pane, surface, "dynamic");
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
        assert!(js.contains("role:\"dynamic\""));
    }

    #[test]
    fn dynamic_preload_injects_context_and_ozmux_bridge() {
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

        let preload = build_dynamic_preload(workspace, pane, surface);
        assert_eq!(preload.0.len(), 2, "context + ozmux bridge");
        assert!(preload.0[0].starts_with("window.__ozmuxContext="));
        assert_eq!(preload.0[1], OZMUX_BRIDGE_JS);
        assert!(
            OZMUX_BRIDGE_JS.contains("window, 'ozmux'")
                && OZMUX_BRIDGE_JS.contains("defineProperty")
        );
        assert!(OZMUX_BRIDGE_JS.contains("kind: 'ozmux.call'"));
    }
}
