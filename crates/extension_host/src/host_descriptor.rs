//! Builds the host-manifest descriptor JSON (consumed by the Node host) and the
//! capability-bearing `ViewRegistry` entries from discovered extensions.

use crate::extension_discovery::DiscoveredExtension;
use crate::registry::{RegisteredView, ViewId};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// One extension's load + serve descriptor, serialized as camelCase to match the
/// Node host's `parseHostManifest` zod schema.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionDescriptorJson {
    /// Extension name (the `ozmux-ext://<name>` host).
    pub name: String,
    /// Absolute paths of the extension's api `.ts` files (traversal-validated).
    pub api_paths: Vec<PathBuf>,
    /// Absolute extension directory the host serves assets from.
    pub asset_root: String,
}

/// The host-manifest JSON Rust writes to `OZMUX_HOST_MANIFEST`.
#[derive(Debug, Clone, Serialize)]
pub struct HostManifestJson {
    /// One descriptor per discovered extension.
    pub extensions: Vec<ExtensionDescriptorJson>,
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
    /// Builds the descriptor JSON + validated view entries from discovered extensions.
    /// A relative path component (`..`) in an api or entry path, and an empty or
    /// whitespace-bearing `view_id`, are rejected (skipped with a warning) — the
    /// trust boundary that keeps manifest data from escaping the extension dir.
    pub fn new(extensions: &[DiscoveredExtension]) -> Self {
        let mut descriptors = Vec::new();
        let mut views = Vec::new();
        for extension in extensions {
            let asset_root = extension.dir.to_string_lossy().into_owned();
            let mut api_paths = Vec::new();
            for rel in &extension.manifest.api {
                if is_safe_rel(rel) {
                    api_paths.push(extension.dir.join(rel));
                } else {
                    bevy::log::warn!(extension = %extension.name, path = %rel.display(), "unsafe api path; skipping");
                }
            }
            descriptors.push(ExtensionDescriptorJson {
                name: extension.name.clone(),
                api_paths,
                asset_root,
            });
            for view in &extension.manifest.views {
                if view.id.as_str().is_empty() || view.id.as_str().chars().any(char::is_whitespace)
                {
                    bevy::log::warn!(extension = %extension.name, id = %view.id.as_str(), "invalid view id; skipping");
                    continue;
                }
                if !is_safe_rel(&view.entry) {
                    bevy::log::warn!(extension = %extension.name, entry = %view.entry.display(), "unsafe view entry; skipping");
                    continue;
                }
                views.push((
                    view.id.clone(),
                    RegisteredView {
                        entry: view.entry.to_string_lossy().into_owned(),
                        owning_ext: extension.name.clone(),
                        interactive: view.interactive,
                        capabilities: view.capabilities.clone(),
                    },
                ));
            }
        }
        Self {
            manifest: HostManifestJson {
                extensions: descriptors,
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
    use crate::extension_manifest::{ExtensionManifest, ExtensionView};
    use std::path::PathBuf;

    fn extension(
        name: &str,
        dir: &str,
        api: &[&str],
        views: Vec<ExtensionView>,
    ) -> DiscoveredExtension {
        DiscoveredExtension {
            name: name.into(),
            dir: PathBuf::from(dir),
            manifest: ExtensionManifest {
                api: api.iter().copied().map(PathBuf::from).collect(),
                views,
            },
        }
    }

    fn view(id: &str, entry: &str, caps: &[&str]) -> ExtensionView {
        ExtensionView {
            id: ViewId::new(id),
            entry: entry.into(),
            capabilities: caps.iter().map(|s| s.to_string()).collect(),
            interactive: true,
        }
    }

    #[test]
    fn builds_camelcase_descriptor_with_absolute_paths() {
        let built =
            BuiltHostManifest::new(&[extension("memo", "/abs/memo", &["api/fs.ts"], vec![])]);
        let json = serde_json::to_string(&built.manifest).unwrap();
        assert_eq!(
            json,
            r#"{"extensions":[{"name":"memo","apiPaths":["/abs/memo/api/fs.ts"],"assetRoot":"/abs/memo"}]}"#
        );
    }

    #[test]
    fn builds_view_entries_with_capabilities() {
        let built = BuiltHostManifest::new(&[extension(
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
        let built = BuiltHostManifest::new(&[extension(
            "bad",
            "/abs/bad",
            &["../escape.ts"],
            vec![view("bad.v", "../../etc/passwd", &[])],
        )]);
        assert!(built.manifest.extensions[0].api_paths.is_empty());
        assert!(built.views.is_empty());
    }

    #[test]
    fn rejects_empty_or_whitespace_view_id() {
        let built = BuiltHostManifest::new(&[extension(
            "p",
            "/abs/p",
            &[],
            vec![view("", "a.html", &[]), view("has space", "b.html", &[])],
        )]);
        assert!(built.views.is_empty());
    }
}
