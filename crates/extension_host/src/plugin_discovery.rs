//! Scans plugin directories for `ozmux.toml`, returning each plugin's parsed
//! manifest. Pure over the given roots; the caller orders roots user-first.

use crate::plugin_manifest::PluginManifest;
use std::path::PathBuf;

/// A discovered plugin: its name (directory name), absolute directory, and parsed manifest.
#[derive(Debug, Clone)]
pub struct DiscoveredPlugin {
    /// Plugin name = its directory name (the `ozmux-ext://<name>` host).
    pub name: String,
    /// Absolute plugin directory (asset root + base for api paths).
    pub dir: PathBuf,
    /// The parsed `ozmux.toml`.
    pub manifest: PluginManifest,
}

/// Scans each root for immediate subdirectories containing an `ozmux.toml`,
/// returning the parsed plugins. Within a root, results are sorted by name;
/// across roots, the first occurrence of a name wins (caller passes user roots
/// first). Unreadable roots and malformed manifests are skipped with a log.
pub fn discover_plugins(roots: &[PathBuf]) -> Vec<DiscoveredPlugin> {
    let mut found = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for root in roots {
        let entries = match std::fs::read_dir(root) {
            Ok(e) => e,
            Err(_) => continue,
        };
        let mut dirs: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
        dirs.sort();
        for dir in dirs {
            let manifest_path = dir.join("ozmux.toml");
            if !manifest_path.is_file() {
                continue;
            }
            let Some(name) = dir.file_name().and_then(|n| n.to_str()).map(str::to_string) else {
                continue;
            };
            let text = match std::fs::read_to_string(&manifest_path) {
                Ok(t) => t,
                Err(e) => {
                    bevy::log::warn!(path = %manifest_path.display(), error = %e, "failed to read ozmux.toml");
                    continue;
                }
            };
            let manifest = match PluginManifest::parse(&text) {
                Ok(m) => m,
                Err(e) => {
                    bevy::log::warn!(path = %manifest_path.display(), error = %e, "failed to parse ozmux.toml");
                    continue;
                }
            };
            if !seen.insert(name.clone()) {
                bevy::log::warn!(name = %name, "duplicate plugin name; keeping first occurrence");
                continue;
            }
            found.push(DiscoveredPlugin {
                name,
                dir,
                manifest,
            });
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use tempfile::tempdir;

    fn write_plugin(root: &Path, name: &str, toml: &str) {
        let dir = root.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("ozmux.toml"), toml).unwrap();
    }

    #[test]
    fn discovers_plugins_with_manifests_sorted() {
        let root = tempdir().unwrap();
        write_plugin(root.path(), "b", "api = [\"a.ts\"]\n");
        write_plugin(root.path(), "a", "api = [\"a.ts\"]\n");
        fs::create_dir_all(root.path().join("no-manifest")).unwrap();
        let found = discover_plugins(&[root.path().to_path_buf()]);
        assert_eq!(
            found.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(found[0].manifest.api, vec![PathBuf::from("a.ts")]);
    }

    #[test]
    fn first_root_wins_on_duplicate_name() {
        let user = tempdir().unwrap();
        let bundled = tempdir().unwrap();
        write_plugin(user.path(), "memo", "api = [\"user.ts\"]\n");
        write_plugin(bundled.path(), "memo", "api = [\"bundled.ts\"]\n");
        let found = discover_plugins(&[user.path().to_path_buf(), bundled.path().to_path_buf()]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].manifest.api, vec![PathBuf::from("user.ts")]);
    }

    #[test]
    fn skips_malformed_manifest() {
        let root = tempdir().unwrap();
        write_plugin(root.path(), "good", "api = [\"a.ts\"]\n");
        write_plugin(root.path(), "bad", "this = = not toml");
        let found = discover_plugins(&[root.path().to_path_buf()]);
        assert_eq!(
            found.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            ["good"]
        );
    }

    #[test]
    fn missing_root_is_ignored() {
        let found = discover_plugins(&[PathBuf::from("/nonexistent-ozmux-root")]);
        assert!(found.is_empty());
    }
}
