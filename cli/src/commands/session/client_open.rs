//! Detached spawn of the `ozmux-client` Tauri launcher with a deep-link
//! URL pointing at a specific session.

use anyhow::{Context, Result};
use std::ffi::OsString;
use std::process::{Command, Stdio};

/// Spawn `ozmux-client` (or whichever binary `OZMUX_CLIENT_BIN` points at)
/// detached from the CLI's controlling tty, passing the session-scoped URL
/// as its first positional argument. Returns an error only when the
/// `spawn()` syscall itself fails (e.g. ENOENT); the child's exit code is
/// not awaited.
pub(super) fn spawn_detached(session_id: &str) -> Result<()> {
    let bin = resolve_client_bin();
    let url = daemon_bootstrap::session_deep_link_url(session_id);

    let mut cmd = Command::new(&bin);
    cmd.arg(&url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    crate::process::detach::configure_detached(&mut cmd);

    cmd.spawn()
        .with_context(|| format!("spawn {} {url}", bin.to_string_lossy()))?;
    // NOTE: drop the child handle without waiting; the launcher is
    // intentionally detached.
    Ok(())
}

fn resolve_client_bin() -> OsString {
    if let Some(v) = std::env::var_os("OZMUX_CLIENT_BIN") {
        return v;
    }
    #[cfg(debug_assertions)]
    if let Some(sibling) = sibling_client_bin() {
        return sibling.into_os_string();
    }
    #[cfg(debug_assertions)]
    if let Some(workspace_built) = workspace_target_debug_client() {
        return workspace_built.into_os_string();
    }
    OsString::from("ozmux-client")
}

#[cfg(debug_assertions)]
fn sibling_client_bin() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    sibling_client_bin_at(exe.parent()?)
}

#[cfg(debug_assertions)]
fn sibling_client_bin_at(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let candidate = dir.join("ozmux-client");
    candidate.is_file().then_some(candidate)
}

#[cfg(debug_assertions)]
fn workspace_target_debug_client() -> Option<std::path::PathBuf> {
    workspace_target_debug_client_in(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))
}

#[cfg(debug_assertions)]
fn workspace_target_debug_client_in(
    cli_manifest_dir: &std::path::Path,
) -> Option<std::path::PathBuf> {
    let candidate = cli_manifest_dir
        .parent()?
        .join("target")
        .join("debug")
        .join("ozmux-client");
    candidate.is_file().then_some(candidate)
}

#[cfg(test)]
mod tests {
    #[test]
    fn deep_link_url_encodes_session_id() {
        let url = daemon_bootstrap::session_deep_link_url("abc-123");
        assert!(url.starts_with("http://"));
        assert!(url.contains("?session=abc%2D123"));
    }

    #[test]
    fn deep_link_url_escapes_dangerous_chars() {
        let url = daemon_bootstrap::session_deep_link_url("hi&id=evil");
        assert!(url.contains("?session=hi%26id%3Devil"));
    }

    #[cfg(debug_assertions)]
    #[test]
    fn sibling_client_bin_at_returns_path_when_file_exists() {
        use std::fs::File;
        let dir = tempfile::tempdir().expect("tempdir");
        let candidate = dir.path().join("ozmux-client");
        File::create(&candidate).expect("create sibling");
        let resolved = super::sibling_client_bin_at(dir.path()).expect("should find sibling");
        assert_eq!(resolved, candidate);
    }

    #[cfg(debug_assertions)]
    #[test]
    fn sibling_client_bin_at_returns_none_when_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(super::sibling_client_bin_at(dir.path()).is_none());
    }

    #[cfg(debug_assertions)]
    #[test]
    fn sibling_client_bin_at_ignores_directory_with_same_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("ozmux-client")).expect("mkdir");
        assert!(super::sibling_client_bin_at(dir.path()).is_none());
    }

    #[cfg(debug_assertions)]
    #[test]
    fn workspace_target_debug_client_in_returns_path_when_file_exists() {
        use std::fs::{File, create_dir_all};
        let workspace = tempfile::tempdir().expect("tempdir");
        let cli_dir = workspace.path().join("cli");
        create_dir_all(&cli_dir).expect("mkdir cli");
        let target_debug = workspace.path().join("target").join("debug");
        create_dir_all(&target_debug).expect("mkdir target/debug");
        let candidate = target_debug.join("ozmux-client");
        File::create(&candidate).expect("create binary");

        let resolved =
            super::workspace_target_debug_client_in(&cli_dir).expect("should find workspace built");
        assert_eq!(resolved, candidate);
    }

    #[cfg(debug_assertions)]
    #[test]
    fn workspace_target_debug_client_in_returns_none_when_missing() {
        let workspace = tempfile::tempdir().expect("tempdir");
        let cli_dir = workspace.path().join("cli");
        std::fs::create_dir(&cli_dir).expect("mkdir cli");
        assert!(super::workspace_target_debug_client_in(&cli_dir).is_none());
    }
}
