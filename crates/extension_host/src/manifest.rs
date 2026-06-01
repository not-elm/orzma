//! Parses an extension's `package.json` for its ozmux manifest fields (name +
//! commands), used by discovery to build a `CommandExtensionConfig`.

use serde::Deserialize;

/// An extension's resolved manifest: its name and the command shims it provides.
#[derive(Debug, Clone)]
pub struct Manifest {
    /// Extension name (from package.json `name`); the `ozmux-ext://<name>` host.
    pub name: String,
    /// Command names whose shims trigger the extension (e.g. `["@memo"]`).
    pub commands: Vec<String>,
}

impl Manifest {
    /// Parses a `package.json` string into a `Manifest`. Errors if `name` is absent.
    pub fn parse(text: &str) -> Result<Self, ManifestError> {
        let raw: RawPackageJson = serde_json::from_str(text).map_err(ManifestError::Json)?;
        let name = raw.name.ok_or(ManifestError::MissingName)?;
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(ManifestError::InvalidName(name));
        }
        Ok(Self {
            name,
            commands: raw.ozmux.unwrap_or_default().commands,
        })
    }
}

/// A failure to parse an extension manifest.
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    /// Malformed `package.json`.
    #[error("invalid package.json: {0}")]
    Json(#[source] serde_json::Error),
    /// `package.json` has no `name`.
    #[error("package.json missing required \"name\"")]
    MissingName,
    /// `name` contains characters unsafe for a path segment / `ozmux-ext://` host.
    #[error("invalid extension name {0:?}: only [A-Za-z0-9_-] allowed")]
    InvalidName(String),
}

#[derive(Deserialize)]
struct RawPackageJson {
    name: Option<String>,
    #[serde(default)]
    ozmux: Option<OzmuxField>,
}

#[derive(Deserialize, Default)]
struct OzmuxField {
    #[serde(default)]
    commands: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_name_and_commands_from_package_json() {
        let json = r#"{ "name": "memo", "ozmux": { "commands": ["@memo"] } }"#;
        let m = Manifest::parse(json).unwrap();
        assert_eq!(m.name, "memo");
        assert_eq!(m.commands, vec!["@memo".to_string()]);
    }

    #[test]
    fn defaults_commands_to_empty_and_requires_name() {
        let m = Manifest::parse(r#"{ "name": "x" }"#).unwrap();
        assert!(m.commands.is_empty());
        assert!(matches!(
            Manifest::parse(r#"{ "ozmux": {} }"#),
            Err(ManifestError::MissingName)
        ));
    }

    #[test]
    fn rejects_malformed_json() {
        assert!(matches!(
            Manifest::parse("{ not json"),
            Err(ManifestError::Json(_))
        ));
    }

    #[test]
    fn rejects_names_unsafe_for_paths_and_urls() {
        for bad in ["../escape", "@scope/pkg", "", "a b"] {
            let json = format!(r#"{{ "name": {bad:?} }}"#);
            assert!(
                matches!(Manifest::parse(&json), Err(ManifestError::InvalidName(_))),
                "name {bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn accepts_alnum_dash_underscore_names() {
        for good in ["memo", "my-ext_2"] {
            let json = format!(r#"{{ "name": {good:?} }}"#);
            assert_eq!(Manifest::parse(&json).unwrap().name, good);
        }
    }
}
