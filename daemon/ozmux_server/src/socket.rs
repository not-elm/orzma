//! The agreed filesystem socket path, shared by the server, the CLI, and the
//! Dart client (which mirrors this formula). Kept in one place because the path
//! is protocol surface.

use interprocess::local_socket::traits::tokio::Stream as _;
use interprocess::local_socket::{GenericFilePath, ToFsName};
use std::path::{Path, PathBuf};

/// Returns the ozmux daemon socket path:
/// `${TMPDIR:-/tmp}/ozmux-<uid>/default.sock`, falling back to a short
/// `/tmp/ozmux-<uid>/` base when `$TMPDIR` is empty/relative or would push the
/// path past the 103-byte usable `sun_path` limit.
pub fn socket_path() -> PathBuf {
    // SAFETY: getuid is always safe; it cannot fail and touches no memory.
    let uid = unsafe { libc::getuid() };
    let tmpdir = std::env::var_os("TMPDIR").map(PathBuf::from);
    resolve_socket_path(tmpdir.as_deref(), uid)
}

/// Returns true if a daemon is already accepting on the shared socket path.
pub async fn socket_is_live() -> bool {
    let Ok(name) = socket_path().to_fs_name::<GenericFilePath>() else {
        return false;
    };
    interprocess::local_socket::tokio::Stream::connect(name)
        .await
        .is_ok()
}

fn resolve_socket_path(tmpdir: Option<&Path>, uid: u32) -> PathBuf {
    let candidate = tmpdir
        .filter(|d| d.is_absolute())
        .map(|d| d.join(format!("ozmux-{uid}")).join("default.sock"));
    match candidate {
        Some(p) if p.as_os_str().len() <= 103 => p,
        _ => PathBuf::from(format!("/tmp/ozmux-{uid}/default.sock")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_ends_with_default_sock_and_uid_dir() {
        // SAFETY: getuid is always safe.
        let uid = unsafe { libc::getuid() };
        let p = socket_path();
        assert!(p.ends_with("default.sock"));
        assert!(
            p.to_string_lossy().contains(&format!("ozmux-{uid}")),
            "path must contain the uid dir: {p:?}"
        );
        assert!(p.as_os_str().len() <= 103, "must fit sun_path: {p:?}");
    }

    #[test]
    fn absolute_short_tmpdir_is_used() {
        let tmpdir = PathBuf::from("/var/folders/ab");
        let p = resolve_socket_path(Some(&tmpdir), 501);
        assert_eq!(p, PathBuf::from("/var/folders/ab/ozmux-501/default.sock"));
    }

    #[test]
    fn missing_tmpdir_falls_back_to_tmp() {
        let p = resolve_socket_path(None, 501);
        assert_eq!(p, PathBuf::from("/tmp/ozmux-501/default.sock"));
    }

    #[test]
    fn overlong_absolute_tmpdir_falls_back_to_tmp() {
        let tmpdir = PathBuf::from(format!("/{}", "x".repeat(200)));
        let p = resolve_socket_path(Some(&tmpdir), 501);
        assert_eq!(p, PathBuf::from("/tmp/ozmux-501/default.sock"));
    }

    #[test]
    fn relative_tmpdir_falls_back_to_tmp() {
        let tmpdir = PathBuf::from("rel");
        let p = resolve_socket_path(Some(&tmpdir), 501);
        assert_eq!(p, PathBuf::from("/tmp/ozmux-501/default.sock"));
    }
}
