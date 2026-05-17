//! Browser storage profile path resolution for cef_host.
//!
//! Maps a `BrowserProfileWire` to a CEF `cache_path`. Named profiles resolve
//! to a sanitized directory under the ozmux data dir; incognito resolves to
//! an empty path (CEF in-memory mode).

use ozmux_browser_cef_protocol::wire::BrowserProfileWire;
use std::path::{Path, PathBuf};

/// Error resolving a browser storage profile.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ProfileError {
    /// Profile name failed validation (empty, traversal, or bad chars).
    #[error("invalid profile name: {0}")]
    InvalidName(String),
}

/// Validates a named-profile name: non-empty, and only `[A-Za-z0-9_-]`.
/// Rejects path separators, `.`/`..`, and anything that could escape the
/// profiles directory.
pub(crate) fn sanitize_profile_name(name: &str) -> Result<(), ProfileError> {
    let ok = !name.is_empty()
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(())
    } else {
        Err(ProfileError::InvalidName(name.to_string()))
    }
}

/// Absolute cache path for a named profile: `<root>/profiles/<name>`.
/// `name` is sanitized first.
pub(crate) fn named_cache_path(root_cache_path: &Path, name: &str) -> Result<PathBuf, ProfileError> {
    sanitize_profile_name(name)?;
    Ok(root_cache_path.join("profiles").join(name))
}

/// Resolved cache directory for a wire profile. `Some(path)` for a named
/// profile (disk-persistent), `None` for incognito (CEF in-memory mode).
pub(crate) fn resolve_cache_path(
    root_cache_path: &Path,
    profile: &BrowserProfileWire,
) -> Result<Option<PathBuf>, ProfileError> {
    match profile {
        BrowserProfileWire::Named { name } => Ok(Some(named_cache_path(root_cache_path, name)?)),
        BrowserProfileWire::Incognito => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn sanitize_accepts_simple_names() {
        assert!(sanitize_profile_name("work").is_ok());
        assert!(sanitize_profile_name("default").is_ok());
        assert!(sanitize_profile_name("customer-a_1").is_ok());
    }

    #[test]
    fn sanitize_rejects_path_traversal() {
        assert!(sanitize_profile_name("../etc").is_err());
        assert!(sanitize_profile_name("a/b").is_err());
        assert!(sanitize_profile_name("").is_err());
        assert!(sanitize_profile_name(".").is_err());
        assert!(sanitize_profile_name("..").is_err());
    }

    #[test]
    fn named_cache_path_is_child_of_root() {
        let root = PathBuf::from("/data/ozmux/browser");
        let p = named_cache_path(&root, "work").unwrap();
        assert_eq!(p, PathBuf::from("/data/ozmux/browser/profiles/work"));
    }

    #[test]
    fn resolve_cache_path_named_and_incognito() {
        let root = PathBuf::from("/data/ozmux/browser");
        let named =
            resolve_cache_path(&root, &BrowserProfileWire::Named { name: "work".into() }).unwrap();
        assert_eq!(named, Some(PathBuf::from("/data/ozmux/browser/profiles/work")));
        let incog = resolve_cache_path(&root, &BrowserProfileWire::Incognito).unwrap();
        assert_eq!(incog, None);
    }
}
