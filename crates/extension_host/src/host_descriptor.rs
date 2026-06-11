//! Builds the host-manifest descriptor JSON (consumed by the Node host) and the
//! capability-bearing `ViewRegistry` entries from discovered plugins.

use crate::plugin_discovery::DiscoveredPlugin;
use crate::registry::{RegisteredView, ViewId};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// One plugin's load + serve descriptor, serialized as camelCase to match the
/// Node host's `parseHostManifest` zod schema.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginDescriptorJson {
    /// Plugin name (the `ozmux-ext://<name>` host).
    pub name: String,
    /// Absolute paths of the plugin's api `.ts` files (traversal-validated).
    pub api_paths: Vec<PathBuf>,
    /// Absolute plugin directory the host serves assets from.
    pub asset_root: String,
}

/// The host-manifest JSON Rust writes to `OZMUX_HOST_MANIFEST`.
#[derive(Debug, Clone, Serialize)]
pub struct HostManifestJson {
    /// One descriptor per discovered plugin.
    pub plugins: Vec<PluginDescriptorJson>,
}

/// The descriptor JSON plus the `ViewRegistry` entries to register.
#[derive(Debug, Clone)]
pub struct BuiltHostManifest {
    /// Serialized to the `OZMUX_HOST_MANIFEST` file for the Node host.
    pub manifest: HostManifestJson,
    /// `(view_id, RegisteredView)` pairs to insert into `ViewRegistry`.
    pub views: Vec<(ViewId, RegisteredView)>,
}

impl BuiltHostManifest {
    /// Builds the descriptor JSON + validated view entries from discovered plugins.
    /// A relative path component (`..`) in an api or entry path, and an empty or
    /// whitespace-bearing `view_id`, are rejected (skipped with a warning) — the
    /// trust boundary that keeps manifest data from escaping the plugin dir.
    pub fn new(plugins: &[DiscoveredPlugin]) -> Self {
        let mut descriptors = Vec::new();
        let mut views = Vec::new();
        for plugin in plugins {
            let asset_root = plugin.dir.to_string_lossy().into_owned();
            let mut api_paths = Vec::new();
            for rel in &plugin.manifest.api {
                if is_safe_rel(rel) {
                    api_paths.push(plugin.dir.join(rel));
                } else {
                    bevy::log::warn!(plugin = %plugin.name, path = %rel.display(), "unsafe api path; skipping");
                }
            }
            descriptors.push(PluginDescriptorJson {
                name: plugin.name.clone(),
                api_paths,
                asset_root,
            });
            for view in &plugin.manifest.views {
                if view.id.as_str().is_empty() || view.id.as_str().chars().any(char::is_whitespace)
                {
                    bevy::log::warn!(plugin = %plugin.name, id = %view.id.as_str(), "invalid view id; skipping");
                    continue;
                }
                if !is_safe_rel(&view.entry) {
                    bevy::log::warn!(plugin = %plugin.name, entry = %view.entry.display(), "unsafe view entry; skipping");
                    continue;
                }
                views.push((
                    view.id.clone(),
                    RegisteredView {
                        entry: view.entry.to_string_lossy().into_owned(),
                        owning_ext: plugin.name.clone(),
                        interactive: view.interactive,
                        capabilities: view.capabilities.clone(),
                    },
                ));
            }
        }
        Self {
            manifest: HostManifestJson {
                plugins: descriptors,
            },
            views,
        }
    }
}

/// True when `rel` is a non-empty relative path made only of normal components
/// (no `..`, no `.`, no leading `/`).
fn is_safe_rel(rel: &Path) -> bool {
    !rel.as_os_str().is_empty()
        && rel
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_manifest::{ManifestView, PluginManifest};
    use std::path::PathBuf;

    fn plugin(name: &str, dir: &str, api: &[&str], views: Vec<ManifestView>) -> DiscoveredPlugin {
        DiscoveredPlugin {
            name: name.into(),
            dir: PathBuf::from(dir),
            manifest: PluginManifest {
                api: api.iter().copied().map(PathBuf::from).collect(),
                views,
            },
        }
    }

    fn view(id: &str, entry: &str, caps: &[&str]) -> ManifestView {
        ManifestView {
            id: ViewId::new(id),
            entry: entry.into(),
            capabilities: caps.iter().map(|s| s.to_string()).collect(),
            interactive: true,
        }
    }

    #[test]
    fn builds_camelcase_descriptor_with_absolute_paths() {
        let built = BuiltHostManifest::new(&[plugin("memo", "/abs/memo", &["api/fs.ts"], vec![])]);
        let json = serde_json::to_string(&built.manifest).unwrap();
        assert_eq!(
            json,
            r#"{"plugins":[{"name":"memo","apiPaths":["/abs/memo/api/fs.ts"],"assetRoot":"/abs/memo"}]}"#
        );
    }

    #[test]
    fn builds_view_entries_with_capabilities() {
        let built = BuiltHostManifest::new(&[plugin(
            "memo",
            "/abs/memo",
            &[],
            vec![view("memo.main", "index.html", &["fs"])],
        )]);
        assert_eq!(built.views.len(), 1);
        let (id, rv) = &built.views[0];
        assert_eq!(id.as_str(), "memo.main");
        assert_eq!(rv.owning_ext, "memo");
        assert_eq!(rv.entry, "index.html");
        assert_eq!(rv.capabilities, vec!["fs".to_string()]);
        assert!(rv.interactive);
    }

    #[test]
    fn rejects_path_traversal_in_entry_and_api() {
        let built = BuiltHostManifest::new(&[plugin(
            "bad",
            "/abs/bad",
            &["../escape.ts"],
            vec![view("bad.v", "../../etc/passwd", &[])],
        )]);
        assert!(built.manifest.plugins[0].api_paths.is_empty());
        assert!(built.views.is_empty());
    }

    #[test]
    fn rejects_empty_or_whitespace_view_id() {
        let built = BuiltHostManifest::new(&[plugin(
            "p",
            "/abs/p",
            &[],
            vec![view("", "a.html", &[]), view("has space", "b.html", &[])],
        )]);
        assert!(built.views.is_empty());
    }
}
