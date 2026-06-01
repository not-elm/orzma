//! Parses an extension's `package.json` for its ozmux manifest field (name),
//! used by discovery to build a `CommandExtensionConfig`.

use serde::Deserialize;

/// An extension's resolved manifest: its name.
#[derive(Debug, Clone)]
pub struct Manifest {
    /// Extension name (from package.json `name`); the `ozmux-ext://<name>` host.
    pub name: String,
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
        Ok(Self { name })
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_name_from_package_json() {
        let json = r#"{ "name": "memo", "ozmux": { "commands": ["@memo"] } }"#;
        let m = Manifest::parse(json).unwrap();
        assert_eq!(m.name, "memo");
    }

    #[test]
    fn requires_name() {
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
