//! Browser storage profile path resolution for cef_host.
//!
//! Maps a `BrowserProfileWire` to a CEF `cache_path`. Named profiles resolve
//! to a sanitized directory under the ozmux data dir; incognito resolves to
//! an empty path (CEF in-memory mode).

use fs2::FileExt;
use ozmux_browser_cef_protocol::wire::BrowserProfileWire;
use std::fs::File;
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
///
/// # NOTE on CEF Chrome runtime profile isolation
///
/// CEF Chrome runtime initializes a "Default" profile directly under
/// `root_cache_path`. A `RequestContext` whose `cache_path` is
/// `profiles/<name>` (two levels deep) is accepted by
/// `request_context_create_context`; Chrome may log a
/// "Cannot create profile at path …/profiles/<name>" diagnostic from
/// `chrome_browser_context.cc`, but `browser_host_create_browser_sync`
/// still succeeds and the context provides per-activity request isolation.
/// Full Chrome-managed profile separation (separate cookie DB, etc.) would
/// require using Chrome's profile naming convention (`Default`, `Profile N`,
/// …) as direct children of `root_cache_path` via the Chrome profile
/// management API — a future improvement tracked separately.
///
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

/// Resolves the CEF `root_cache_path` from explicit env values. Prefers
/// `$XDG_DATA_HOME/ozmux/browser`, else `<home>/.local/share/ozmux/browser`.
pub fn browser_data_root_from(xdg_data_home: Option<&str>, home: &str) -> PathBuf {
    match xdg_data_home.filter(|s| !s.is_empty()) {
        Some(xdg) => PathBuf::from(xdg).join("ozmux").join("browser"),
        None => PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("ozmux")
            .join("browser"),
    }
}

/// Resolves the browser data root from the process environment.
pub fn browser_data_root() -> PathBuf {
    // NOTE: HOME is effectively always set for cef_host (it is launched inside
    // a user session). The /tmp fallback is a last resort so the process does
    // not panic on a degenerate environment; storage there is world-readable
    // and reboot-cleared, which is acceptable only because HOME being unset is
    // a pathological case.
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    browser_data_root_from(std::env::var("XDG_DATA_HOME").ok().as_deref(), &home)
}

/// An exclusive lock on the browser data root, held for the daemon's life.
/// While held, this daemon owns disk-persistent profiles.
pub struct DataRootLock {
    // NOTE: held only for its Drop side-effect — closing the file releases the
    // fs2 advisory lock.
    _file: File,
}

/// Attempts to take the data-root lock. `Some` → this daemon owns persistent
/// profiles. `None` → another daemon holds it; persistence must be disabled.
///
/// A genuine I/O error (not lock contention) propagates as `Err` so the caller
/// surfaces it rather than silently degrading to disabled persistence.
pub fn acquire_data_root_lock(root: &Path) -> std::io::Result<Option<DataRootLock>> {
    std::fs::create_dir_all(root)?;
    let file = File::create(root.join("lock"))?;
    match file.try_lock_exclusive() {
        Ok(()) => Ok(Some(DataRootLock { _file: file })),
        Err(e) if e.kind() == fs2::lock_contended_error().kind() => Ok(None),
        Err(e) => Err(e),
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
    fn data_root_uses_xdg_data_home_when_set() {
        let root = browser_data_root_from(Some("/custom/xdg"), "/home/u");
        assert_eq!(root, PathBuf::from("/custom/xdg/ozmux/browser"));
    }

    #[test]
    fn data_root_falls_back_to_home_local_share() {
        let root = browser_data_root_from(None, "/home/u");
        assert_eq!(root, PathBuf::from("/home/u/.local/share/ozmux/browser"));
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
