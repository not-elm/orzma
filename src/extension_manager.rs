//! Discovers ozmux Node extensions from the bundled + user extension roots,
//! launches each as a `CommandExtension`, and owns the live per-extension
//! registry (process handles + the shared `ozmux-ext://` asset
//! `EndpointRegistry`). Replaces the old single hardcoded memo wiring: the
//! renderer routes per extension off each surface's `OwningExtension`, and the
//! control bridge drain runs across every launched extension.

use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use ozmux_configs::path::{SystemEnv, extensions_dir, plugins_dir};
use ozmux_extension_host::host::{
    EndpointRegistry, ExtensionEndpoints, LifecycleEvent, RuntimeRoot,
};
use ozmux_extension_host::{
    BuiltHostManifest, CommandExtension, CommandExtensionConfig, ExtensionControlSet, HostProcess,
    Manifest, RegisteredView, ViewId, ViewRegistry, apply_control_request, discover_plugins,
};
use ozmux_multiplexer::MultiplexerCommands;
use std::collections::HashSet;
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

const EXTENSION_MAIN: &str = "bootstrap.ts";
const PACKAGE_JSON: &str = "package.json";
const READY_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Resource)]
struct HostRuntime {
    host: HostProcess,
}

struct DiscoveredExtension {
    config: CommandExtensionConfig,
}

/// The live set of launched extensions plus the shared `ozmux-ext://` asset
/// endpoint registry the CEF scheme handler dispatches through.
#[derive(Resource)]
pub(crate) struct ExtensionRegistry {
    /// Extension name → its running process handle.
    pub(crate) extensions: HashMap<String, CommandExtension>,
    /// Shared name → asset-endpoint map read by the `ozmux-ext://` scheme. The
    /// scheme handler reads its own clone (passed to `cef_plugin` in `main`);
    /// the resource holds the canonical handle so it stays alive for the app's
    /// lifetime, and `publish_ready_endpoints` writes each extension's live
    /// asset socket path into it once the extension signals readiness.
    endpoints: EndpointRegistry,
}

/// Discovers + launches every extension at Startup and drains each launched
/// extension's control socket into the multiplexer every frame.
pub(crate) struct ExtensionManagerPlugin {
    endpoints: EndpointRegistry,
}

impl ExtensionManagerPlugin {
    /// Builds the plugin sharing `endpoints` with the CEF scheme handler so the
    /// handler reads the very registry the manager populates on launch.
    pub(crate) fn new(endpoints: EndpointRegistry) -> Self {
        Self { endpoints }
    }
}

impl Plugin for ExtensionManagerPlugin {
    fn build(&self, app: &mut App) {
        let roots = discovery_roots();
        let found = discover_extensions(&roots);
        let mut extensions = HashMap::new();
        let endpoints = self.endpoints.clone();
        for d in found {
            let name = d.config.name.clone();
            match CommandExtension::spawn(d.config) {
                Ok(ext) => {
                    // NOTE: register the name with an EMPTY endpoint at spawn so an
                    // early CEF fetch resolves the name but finds no socket yet
                    // (FetchError::NotReady → 503), instead of hitting ECONNREFUSED
                    // on a socket the child has not bound (502). The real socket
                    // path is published by `publish_ready_endpoints` on readiness.
                    endpoints.insert(name.clone(), ExtensionEndpoints::default());
                    extensions.insert(name, ext);
                }
                Err(e) => {
                    tracing::error!(extension = %name, error = %e, "failed to launch extension");
                }
            }
        }

        app.insert_resource(ExtensionRegistry {
            extensions,
            endpoints,
        });
        let plugins = discover_plugins(&plugin_roots());
        let built = BuiltHostManifest::new(&plugins);
        let descriptor_json =
            serde_json::to_string(&built.manifest).expect("host manifest serializes");
        {
            let mut view_registry = app.world_mut().get_resource_or_init::<ViewRegistry>();
            register_views(&mut view_registry, built.views);
        }
        let host_entry: OsString = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("sdk/typescript/src/host/main.ts")
            .into_os_string();
        match RuntimeRoot::resolve_in(&std::env::temp_dir(), std::process::id(), "host")
            .map_err(|e| e.to_string())
            .and_then(|rt| {
                HostProcess::spawn(host_entry, rt, &descriptor_json, READY_TIMEOUT)
                    .map_err(|e| e.to_string())
            }) {
            Ok(host) => {
                for plugin in &plugins {
                    // NOTE: coexistence slice — a plugin sharing a name with a
                    // launched legacy extension would clobber its asset endpoint
                    // (last-write-wins). Skip + warn; Step 5 removes the legacy half.
                    if self.endpoints.get(&plugin.name).is_some() {
                        tracing::warn!(name = %plugin.name, "plugin name collides with a legacy extension; skipping");
                        continue;
                    }
                    self.endpoints
                        .insert(plugin.name.clone(), ExtensionEndpoints::default());
                }
                app.insert_resource(HostRuntime { host });
            }
            Err(e) => tracing::error!(error = %e, "failed to spawn single host process"),
        }
        app.add_systems(
            Update,
            (
                drain_all_control_requests.in_set(ExtensionControlSet::Drain),
                publish_ready_endpoints,
                poll_host_lifecycle,
            ),
        );
    }
}

/// Resolves the directories ozmux scans for extensions: the bundled
/// `extensions/` dir (next to the binary's manifest) plus the user extensions
/// dir from `ozmux_configs`. The user dir is skipped (with a warning) when it
/// cannot be resolved.
fn discovery_roots() -> Vec<PathBuf> {
    let bundled = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("extensions");
    let mut roots = vec![bundled];
    match extensions_dir(&SystemEnv) {
        Ok(dir) => roots.push(dir),
        Err(e) => {
            tracing::warn!(error = %e, "could not resolve user extensions dir; scanning bundled only")
        }
    }
    roots
}

/// Scans each root for subdirectories carrying a `package.json`, parses each as
/// a manifest, and builds one `DiscoveredExtension` per valid manifest. The
/// entry script is fixed to `bootstrap.ts`. Names are deduplicated across all
/// roots (first occurrence wins; later duplicates are skipped). Directory
/// entries are sorted for deterministic ordering; subdirs lacking a
/// `package.json` or with a parse error are skipped with a warning.
fn discover_extensions(roots: &[PathBuf]) -> Vec<DiscoveredExtension> {
    let mut found = Vec::new();
    let mut seen = HashSet::new();
    for root in roots {
        let entries = match std::fs::read_dir(root) {
            Ok(e) => e,
            Err(_) => continue,
        };
        let mut dirs: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
        dirs.sort();
        for dir in dirs {
            let manifest_path = dir.join(PACKAGE_JSON);
            if !manifest_path.is_file() {
                continue;
            }
            let text = match std::fs::read_to_string(&manifest_path) {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!(path = %manifest_path.display(), error = %e, "failed to read extension package.json");
                    continue;
                }
            };
            let manifest = match Manifest::parse(&text) {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!(path = %manifest_path.display(), error = %e, "failed to parse extension package.json");
                    continue;
                }
            };
            if !seen.insert(manifest.name.clone()) {
                tracing::warn!(name = %manifest.name, "duplicate extension name; skipping later occurrence");
                continue;
            }
            found.push(DiscoveredExtension {
                config: CommandExtensionConfig {
                    name: manifest.name,
                    dir,
                    main: EXTENSION_MAIN.into(),
                },
            });
        }
    }
    found
}

fn drain_all_control_requests(
    mut view_registry: ResMut<ViewRegistry>,
    registry: Res<ExtensionRegistry>,
    mut mux: MultiplexerCommands,
) {
    for (name, ext) in registry.extensions.iter() {
        while let Ok((req, responder)) = ext.control_requests().try_recv() {
            apply_control_request(&mut mux, &mut view_registry, name, req, responder);
        }
    }
}

fn publish_ready_endpoints(registry: Res<ExtensionRegistry>) {
    for (name, ext) in registry.extensions.iter() {
        while let Ok(event) = ext.events().try_recv() {
            if let LifecycleEvent::Ready = event
                && let Some(ep) = registry.endpoints.get(name)
            {
                ep.set(ext.asset_sock_path().to_path_buf());
            }
        }
    }
}

fn poll_host_lifecycle(host: Option<Res<HostRuntime>>) {
    let Some(host) = host else {
        return;
    };
    while let Ok(event) = host.host.events().try_recv() {
        match event {
            LifecycleEvent::Ready => tracing::info!("single host process ready"),
            LifecycleEvent::SpawnFailed { error } => {
                tracing::error!(%error, "single host failed to become ready")
            }
            LifecycleEvent::Exited { status } => {
                tracing::warn!(?status, "single host process exited")
            }
        }
    }
}

fn plugin_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    match plugins_dir(&SystemEnv) {
        Ok(dir) => roots.push(dir),
        Err(e) => tracing::warn!(error = %e, "could not resolve user plugins dir"),
    }
    roots.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("plugins"));
    roots
}

fn register_views(registry: &mut ViewRegistry, views: Vec<(ViewId, RegisteredView)>) {
    for (view_id, view) in views {
        registry.register(view_id.into_inner(), view);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn memo_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("extensions/memo")
    }

    fn node_and_memo_available() -> bool {
        let node = std::process::Command::new("sh")
            .arg("-c")
            .arg("command -v node")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        node && memo_dir().join("node_modules/@ozmux/sdk").exists()
    }

    #[test]
    fn endpoint_stays_unpublished_until_extension_is_ready() {
        if !node_and_memo_available() {
            eprintln!("skipping: node or memo's @ozmux/sdk link not available");
            return;
        }
        let ext = CommandExtension::spawn_with_timeout(
            CommandExtensionConfig {
                name: "memo".into(),
                dir: memo_dir(),
                main: EXTENSION_MAIN.into(),
            },
            Duration::from_secs(20),
        )
        .expect("spawn memo");

        let endpoints = EndpointRegistry::default();
        endpoints.insert("memo", ExtensionEndpoints::default());
        let mut extensions: HashMap<String, CommandExtension> = HashMap::new();
        extensions.insert("memo".into(), ext);

        let registered = endpoints.get("memo").expect("name resolves at spawn");
        assert!(
            registered.get().is_none(),
            "before readiness the endpoint must resolve the name but have no socket (NotReady -> 503, not 502)"
        );

        let mut app = App::new();
        app.insert_resource(ExtensionRegistry {
            extensions,
            endpoints: endpoints.clone(),
        });
        app.add_systems(Update, publish_ready_endpoints);

        let deadline = std::time::Instant::now() + Duration::from_secs(25);
        loop {
            app.update();
            if endpoints.get("memo").and_then(|ep| ep.get()).is_some() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "asset endpoint was never published after readiness"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn discovers_dirs_with_package_json() {
        let tmp = tempfile::tempdir().unwrap();
        let mk = |name: &str| {
            let d = tmp.path().join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(
                d.join("package.json"),
                format!(r#"{{"name":"{name}","main":"bootstrap.ts"}}"#),
            )
            .unwrap();
        };
        mk("memo");
        mk("note");
        std::fs::create_dir_all(tmp.path().join("not-an-ext")).unwrap();
        let found = discover_extensions(&[tmp.path().to_path_buf()]);
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn dedups_by_name_across_roots_first_wins() {
        let root_a = tempfile::tempdir().unwrap();
        let root_b = tempfile::tempdir().unwrap();
        let mk = |root: &std::path::Path, name: &str| {
            let d = root.join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("package.json"), format!(r#"{{"name":"{name}"}}"#)).unwrap();
        };
        mk(root_a.path(), "memo");
        mk(root_b.path(), "memo");
        let found =
            discover_extensions(&[root_a.path().to_path_buf(), root_b.path().to_path_buf()]);
        assert_eq!(
            found.len(),
            1,
            "duplicate name across roots collapses to one"
        );
        assert_eq!(found[0].config.name, "memo");
    }

    #[test]
    fn fixes_main_to_bootstrap_ts() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().join("memo");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("package.json"),
            r#"{"name":"memo","main":"other.js"}"#,
        )
        .unwrap();
        let found = discover_extensions(&[tmp.path().to_path_buf()]);
        assert_eq!(
            found[0].config.main,
            std::ffi::OsString::from("bootstrap.ts")
        );
    }
}
