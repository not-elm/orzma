//! Discovers ozmux Node extensions from the bundled + user extension roots,
//! launches each as a `CommandExtension`, and owns the live per-extension
//! registry (process handles + the shared `ozmux-ext://` asset
//! `EndpointRegistry`). Replaces the old single hardcoded memo wiring: the
//! renderer routes per extension off each activity's `OwningExtension`, and the
//! control bridge drain runs across every launched extension.

use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use ozmux_configs::path::{SystemEnv, extensions_dir};
use ozmux_extension_host::host::{EndpointRegistry, ExtensionEndpoints};
use ozmux_extension_host::{
    CommandExtension, CommandExtensionConfig, ExtensionControlSet, Manifest, apply_control_request,
};
use ozmux_multiplexer::MultiplexerCommands;
use std::collections::HashSet;
use std::path::PathBuf;

const EXTENSION_MAIN: &str = "bootstrap.ts";
const PACKAGE_JSON: &str = "package.json";

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
    /// lifetime.
    // NOTE: the scheme handler holds a separate clone, so the in-resource handle
    // is written (populated on launch) but never read back here; it exists to
    // own the shared registry for the world's lifetime.
    #[expect(
        dead_code,
        reason = "canonical owner of the shared endpoint registry; scheme handler reads its own clone"
    )]
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
        for cmd in command_collisions(&found) {
            tracing::warn!(
                command = %cmd,
                "extension command declared by more than one extension; the @<cmd> shim is ambiguous"
            );
        }

        let mut extensions = HashMap::new();
        let endpoints = self.endpoints.clone();
        for d in found {
            let name = d.config.name.clone();
            match CommandExtension::spawn(d.config) {
                Ok(ext) => {
                    let ep = ExtensionEndpoints::default();
                    ep.set(ext.asset_sock_path().to_path_buf());
                    endpoints.insert(name.clone(), ep);
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
        app.add_systems(
            Update,
            drain_all_control_requests.in_set(ExtensionControlSet::Drain),
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
                    commands: manifest.commands,
                },
            });
        }
    }
    found
}

/// Returns the command names declared by more than one discovered extension.
/// Such commands map to an ambiguous `@<cmd>` shim, so the caller warns.
fn command_collisions(found: &[DiscoveredExtension]) -> Vec<String> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for d in found {
        for cmd in &d.config.commands {
            *counts.entry(cmd.clone()).or_insert(0) += 1;
        }
    }
    let mut collisions: Vec<String> = counts
        .into_iter()
        .filter_map(|(cmd, n)| (n > 1).then_some(cmd))
        .collect();
    collisions.sort();
    collisions
}

fn drain_all_control_requests(registry: Res<ExtensionRegistry>, mut mux: MultiplexerCommands) {
    for ext in registry.extensions.values() {
        while let Ok((req, responder)) = ext.control_requests().try_recv() {
            apply_control_request(&mut mux, req, responder);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_dirs_with_package_json_and_detects_command_collisions() {
        let tmp = tempfile::tempdir().unwrap();
        let mk = |name: &str, cmds: &str| {
            let d = tmp.path().join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(
                d.join("package.json"),
                format!(
                    r#"{{"name":"{name}","main":"bootstrap.ts","ozmux":{{"commands":[{cmds}]}}}}"#
                ),
            )
            .unwrap();
        };
        mk("memo", "\"@memo\"");
        mk("note", "\"@note\"");
        std::fs::create_dir_all(tmp.path().join("not-an-ext")).unwrap();
        let found = discover_extensions(&[tmp.path().to_path_buf()]);
        assert_eq!(found.len(), 2);

        let tmp2 = tempfile::tempdir().unwrap();
        let mk2 = |name: &str| {
            let d = tmp2.path().join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(
                d.join("package.json"),
                format!(r#"{{"name":"{name}","ozmux":{{"commands":["@dup"]}}}}"#),
            )
            .unwrap();
        };
        mk2("a");
        mk2("b");
        assert!(
            command_collisions(&discover_extensions(&[tmp2.path().to_path_buf()]))
                .contains(&"@dup".to_string())
        );
    }

    #[test]
    fn dedups_by_name_across_roots_first_wins() {
        let root_a = tempfile::tempdir().unwrap();
        let root_b = tempfile::tempdir().unwrap();
        let mk = |root: &std::path::Path, name: &str, cmd: &str| {
            let d = root.join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(
                d.join("package.json"),
                format!(r#"{{"name":"{name}","ozmux":{{"commands":["{cmd}"]}}}}"#),
            )
            .unwrap();
        };
        mk(root_a.path(), "memo", "@memo");
        mk(root_b.path(), "memo", "@other");
        let found =
            discover_extensions(&[root_a.path().to_path_buf(), root_b.path().to_path_buf()]);
        assert_eq!(
            found.len(),
            1,
            "duplicate name across roots collapses to one"
        );
        assert_eq!(found[0].config.commands, vec!["@memo".to_string()]);
    }

    #[test]
    fn fixes_main_to_bootstrap_ts() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().join("memo");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("package.json"),
            r#"{"name":"memo","main":"other.js","ozmux":{"commands":["@memo"]}}"#,
        )
        .unwrap();
        let found = discover_extensions(&[tmp.path().to_path_buf()]);
        assert_eq!(
            found[0].config.main,
            std::ffi::OsString::from("bootstrap.ts")
        );
    }

    #[test]
    fn no_collisions_when_commands_are_unique() {
        let tmp = tempfile::tempdir().unwrap();
        let mk = |name: &str, cmd: &str| {
            let d = tmp.path().join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(
                d.join("package.json"),
                format!(r#"{{"name":"{name}","ozmux":{{"commands":["{cmd}"]}}}}"#),
            )
            .unwrap();
        };
        mk("memo", "@memo");
        mk("note", "@note");
        assert!(command_collisions(&discover_extensions(&[tmp.path().to_path_buf()])).is_empty());
    }
}
