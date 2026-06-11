//! Parses a plugin's `ozmux.toml` manifest: the views it publishes for OSC
//! mounting and the host-API capabilities each view is granted.

use crate::error::ExtensionError;
use crate::registry::ViewId;
use serde::Deserialize;
use std::path::PathBuf;

/// A plugin's resolved manifest: the views it publishes for OSC mounting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginManifest {
    /// Plugin-relative paths of the api `.ts` files this plugin loads (multiple allowed).
    pub api: Vec<PathBuf>,
    /// Views this plugin publishes, addressable by `view_id` from OSC mounts.
    pub views: Vec<ManifestView>,
}

/// One view a plugin publishes for OSC mounting, with its capability grant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestView {
    /// PTY-facing identifier referenced by `OSC mount;<id>`.
    pub id: ViewId,
    /// HTML entry path relative to the plugin dir (e.g. `index.html`).
    pub entry: PathBuf,
    /// Host-API namespaces this view's webview may call (namespace granularity).
    pub capabilities: Vec<String>,
    /// Whether the mounted webview accepts pointer/keyboard input.
    pub interactive: bool,
}

impl PluginManifest {
    /// Parses an `ozmux.toml` string into a `PluginManifest`.
    pub fn parse(text: &str) -> Result<Self, ExtensionError> {
        let raw: RawManifest = toml::from_str(text).map_err(ExtensionError::Toml)?;
        let views = raw
            .views
            .into_iter()
            .map(|v| ManifestView {
                id: ViewId::new(v.id),
                entry: v.entry,
                capabilities: v.capabilities,
                interactive: v.interactive,
            })
            .collect();
        Ok(Self {
            api: raw.api,
            views,
        })
    }
}

#[derive(Deserialize)]
struct RawManifest {
    #[serde(default)]
    api: Vec<PathBuf>,
    #[serde(default)]
    views: Vec<RawView>,
}

#[derive(Deserialize)]
struct RawView {
    id: String,
    entry: PathBuf,
    #[serde(default)]
    capabilities: Vec<String>,
    #[serde(default)]
    interactive: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_view_with_capabilities() {
        let text = r#"
[[views]]
id = "memo.main"
entry = "index.html"
capabilities = ["fs"]
interactive = true
"#;
        let m = PluginManifest::parse(text).unwrap();
        assert_eq!(m.views.len(), 1);
        let v = &m.views[0];
        assert_eq!(v.id.as_str(), "memo.main");
        assert_eq!(v.entry, PathBuf::from("index.html"));
        assert_eq!(v.capabilities, vec!["fs".to_string()]);
        assert!(v.interactive);
    }

    #[test]
    fn capabilities_and_interactive_default_to_empty_and_false() {
        let text = r#"
[[views]]
id = "v"
entry = "a.html"
"#;
        let v = &PluginManifest::parse(text).unwrap().views[0];
        assert!(v.capabilities.is_empty());
        assert!(!v.interactive);
    }

    #[test]
    fn empty_text_has_no_views() {
        assert!(PluginManifest::parse("").unwrap().views.is_empty());
    }

    #[test]
    fn missing_required_field_errors() {
        let text = r#"
[[views]]
entry = "a.html"
"#;
        assert!(matches!(
            PluginManifest::parse(text),
            Err(ExtensionError::Toml(_))
        ));
    }

    #[test]
    fn rejects_malformed_toml() {
        assert!(matches!(
            PluginManifest::parse("[[views]"),
            Err(ExtensionError::Toml(_))
        ));
    }

    #[test]
    fn parses_plugin_level_api_files() {
        let text = r#"
api = ["api/fs.ts", "api/net.ts"]

[[views]]
id = "memo.main"
entry = "index.html"
"#;
        let m = PluginManifest::parse(text).unwrap();
        assert_eq!(
            m.api,
            vec![PathBuf::from("api/fs.ts"), PathBuf::from("api/net.ts")]
        );
        assert_eq!(m.views.len(), 1);
    }

    #[test]
    fn api_defaults_to_empty() {
        let m = PluginManifest::parse("[[views]]\nid = \"v\"\nentry = \"a.html\"\n").unwrap();
        assert!(m.api.is_empty());
    }
}
