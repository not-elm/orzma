//! The GUI's copy of the host's registered assets, which the `orzma://`
//! scheme handler serves from.

use crate::webview::scheme::WebviewAssetRegistry;
use bevy::prelude::*;
use bevy_orzmux::prelude::OrzmuxWebviewEvent;
use orzma_webview_host::prelude::{WebviewAsset, WebviewEvent};

/// Keeps the registry the `orzma://` scheme handler reads in step with the
/// host's `AssetRegistered` and `AssetReleased` events.
pub(crate) struct AssetsPlugin {
    registry: WebviewAssetRegistry,
}

impl AssetsPlugin {
    /// A plugin that mirrors the host's assets into `registry`, the registry
    /// the `orzma://` scheme handler was built with.
    pub fn new(registry: WebviewAssetRegistry) -> Self {
        Self { registry }
    }
}

impl Plugin for AssetsPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(AssetMirror(self.registry.clone()))
            .add_observer(apply_asset_event);
    }
}

/// The registry the `orzma://` scheme handler reads.
#[derive(Resource)]
struct AssetMirror(WebviewAssetRegistry);

/// Adds the asset an `AssetRegistered` names, or removes the one an
/// `AssetReleased` names.
fn apply_asset_event(ev: On<OrzmuxWebviewEvent>, mirror: Res<AssetMirror>) {
    match ev.webview_event() {
        WebviewEvent::AssetRegistered { handle, asset } => match asset {
            WebviewAsset::Dir(root) => mirror.0.insert_dir(handle.as_str(), root.clone()),
            WebviewAsset::Inline(html) => mirror.0.insert_inline(handle.as_str(), html.clone()),
        },
        WebviewEvent::AssetReleased { handle } => mirror.0.remove(handle.as_str()),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orzma_webview_host::prelude::HandleId;
    use orzmux::prelude::CommandSeq;
    use std::path::PathBuf;

    fn app(registry: WebviewAssetRegistry) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(AssetsPlugin::new(registry));
        app
    }

    fn host_event(app: &mut App, event: WebviewEvent<Entity>) {
        app.world_mut()
            .trigger(OrzmuxWebviewEvent::new(event, CommandSeq(0)));
        app.world_mut().flush();
    }

    /// Asserts that an inline page is served from the moment it is
    /// registered until it is released.
    ///
    /// Case: a program registers an inline page and later unregisters it.
    #[test]
    fn an_inline_asset_is_served_until_its_release() {
        let registry = WebviewAssetRegistry::default();
        let mut app = app(registry.clone());
        host_event(
            &mut app,
            WebviewEvent::AssetRegistered {
                handle: HandleId::from("h"),
                asset: WebviewAsset::Inline(b"<h1>x</h1>".to_vec()),
            },
        );
        assert_eq!(
            registry.get("h"),
            Some(WebviewAsset::Inline(b"<h1>x</h1>".to_vec()))
        );
        host_event(
            &mut app,
            WebviewEvent::AssetReleased {
                handle: HandleId::from("h"),
            },
        );
        assert_eq!(registry.get("h"), None);
    }

    /// Asserts that a registered directory is served from its root.
    ///
    /// Case: orzmd registers the directory that holds its built UI.
    #[test]
    fn a_directory_asset_is_served_from_its_root() {
        let registry = WebviewAssetRegistry::default();
        let mut app = app(registry.clone());
        host_event(
            &mut app,
            WebviewEvent::AssetRegistered {
                handle: HandleId::from("d"),
                asset: WebviewAsset::Dir(PathBuf::from("/abs/ui")),
            },
        );
        assert_eq!(
            registry.get("d"),
            Some(WebviewAsset::Dir(PathBuf::from("/abs/ui")))
        );
    }
}
