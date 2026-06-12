//! Discovers ozmux Node extensions from the bundled + user extension roots,
//! launches them under the single host process, and owns the live
//! `ozmux-ext://` asset `AssetSourceRegistry`.

use crate::extension_render::HostRpc;
use bevy::prelude::*;
use ozmux_configs::path::{SystemEnv, extensions_dir};
use ozmux_extension_host::host::{AssetSource, AssetSourceRegistry, LifecycleEvent, RuntimeRoot};
use ozmux_extension_host::{
    BuiltHostManifest, HostProcess, HostRpcClient, RegisteredView, ViewId, ViewRegistry,
    discover_extensions,
};
use std::path::PathBuf;
use std::time::Duration;

const READY_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Resource)]
struct HostRuntime {
    host: HostProcess,
}

/// Discovers + launches every extension under the single host process at
/// Startup and polls its lifecycle every frame.
pub(crate) struct ExtensionManagerPlugin {
    endpoints: AssetSourceRegistry,
}

impl ExtensionManagerPlugin {
    /// Builds the extension sharing `endpoints` with the CEF scheme handler so the
    /// handler reads the very registry the manager populates on launch.
    pub(crate) fn new(endpoints: AssetSourceRegistry) -> Self {
        Self { endpoints }
    }

    fn spawn_single_host(&self, app: &mut App) {
        let extensions = discover_extensions(&extension_roots());
        let built = BuiltHostManifest::new(&extensions);
        let descriptor_json =
            serde_json::to_string(&built.manifest).expect("host manifest serializes");
        {
            let mut view_registry = app.world_mut().get_resource_or_init::<ViewRegistry>();
            register_views(&mut view_registry, built.views);
        }
        match RuntimeRoot::resolve_in(&std::env::temp_dir(), std::process::id(), "host")
            .map_err(|e| e.to_string())
            .and_then(|rt| {
                HostProcess::spawn(rt, &descriptor_json, READY_TIMEOUT).map_err(|e| e.to_string())
            }) {
            Ok(host) => {
                for extension in &extensions {
                    self.endpoints.insert(
                        extension.name.clone(),
                        AssetSource::Static(extension.dir.clone()),
                    );
                }
                app.insert_resource(HostRuntime { host });
            }
            Err(e) => tracing::error!(error = %e, "failed to spawn single host process"),
        }
    }
}

impl Plugin for ExtensionManagerPlugin {
    fn build(&self, app: &mut App) {
        self.spawn_single_host(app);
        app.add_systems(Update, poll_host_lifecycle);
    }
}

/// Roots scanned for the single host's extensions: the user dir always, plus
/// the project-root bundled `extensions/` only under the `debug` feature.
fn extension_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    match extensions_dir(&SystemEnv) {
        Ok(dir) => roots.push(dir),
        Err(e) => tracing::warn!(error = %e, "could not resolve user extensions dir"),
    }
    // NOTE: the project-root bundled `extensions/` is dev-only — it is baked at
    // compile time (CARGO_MANIFEST_DIR) and absent from a shipped binary.
    #[cfg(feature = "debug")]
    roots.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("extensions"));
    roots
}

fn register_views(registry: &mut ViewRegistry, views: Vec<(ViewId, RegisteredView)>) {
    for (view_id, view) in views {
        registry.register(view_id.into_inner(), view);
    }
}

fn poll_host_lifecycle(mut host_rpc: Option<ResMut<HostRpc>>, host: Option<Res<HostRuntime>>) {
    let Some(host) = host else {
        return;
    };
    while let Ok(event) = host.host.events().try_recv() {
        match event {
            LifecycleEvent::Ready => match HostRpcClient::connect(host.host.rpc_sock_path()) {
                Ok(client) => {
                    tracing::info!("single host process ready; RPC connected");
                    if let Some(hr) = host_rpc.as_mut() {
                        hr.set_client(client);
                    }
                }
                Err(error) => {
                    tracing::error!(%error, "single host ready but RPC connect failed");
                    if let Some(hr) = host_rpc.as_mut() {
                        hr.clear_client();
                    }
                }
            },
            LifecycleEvent::SpawnFailed { error } => {
                tracing::error!(%error, "single host failed to become ready");
                if let Some(hr) = host_rpc.as_mut() {
                    hr.clear_client();
                }
            }
            LifecycleEvent::Exited { status } => {
                tracing::warn!(?status, "single host process exited");
                if let Some(hr) = host_rpc.as_mut() {
                    hr.clear_client();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ozmux_extension_host::ExtensionManifest;

    #[test]
    fn register_views_populates_registry_with_capabilities() {
        let mut reg = ViewRegistry::default();
        register_views(
            &mut reg,
            vec![(
                ViewId::new("memo.main"),
                RegisteredView {
                    entry: "index.html".into(),
                    owning_ext: "memo".into(),
                    interactive: true,
                    capabilities: vec!["fs".into()],
                },
            )],
        );
        let v = reg.get("memo.main").expect("registered");
        assert_eq!(v.capabilities, vec!["fs".to_string()]);
        assert_eq!(v.owning_ext, "memo");
    }

    #[test]
    fn bundled_memo_manifest_publishes_memo_main_with_fs_capability() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("extensions/memo/ozmux.toml");
        let toml = std::fs::read_to_string(&path).expect("memo ozmux.toml exists");
        let m = ExtensionManifest::parse(&toml).expect("memo ozmux.toml parses");
        assert_eq!(m.api, vec![PathBuf::from("api.ts")], "memo declares api.ts");
        assert_eq!(m.views.len(), 1, "memo publishes exactly one view");
        let v = &m.views[0];
        assert_eq!(v.id.as_str(), "memo.main");
        assert_eq!(v.entry, PathBuf::from("index.html"));
        assert_eq!(v.capabilities, vec!["fs".to_string()]);
        assert!(v.interactive, "memo.main is interactive");
    }

    #[test]
    fn clearing_the_host_client_drops_stale_in_flight_correlation() {
        use crate::extension_render::HostRpc;
        let mut hr = HostRpc::default();
        hr.note_in_flight_for_test("0", bevy::prelude::Entity::PLACEHOLDER, "h0");
        assert_eq!(hr.count_in_flight_for_test(), 1);
        hr.clear_client();
        assert_eq!(
            hr.count_in_flight_for_test(),
            0,
            "clear_client wipes stale correlation"
        );
    }
}
