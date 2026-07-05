//! Stages local image files referenced by a Markdown document as symlinks
//! under the served asset root's `_local/` directory, so the webview can load
//! them as `orzma://<handle>/_local/<token>.<ext>` subresources.

use crate::document::resolve_link;
use std::fs;
use std::io::ErrorKind;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::symlink;
use std::path::Path;

/// Stages the local file `raw` (resolved against `base_dir`) as a symlink under
/// `local_root/_local/` and returns its root-relative served URL, or `None` if
/// `raw` does not resolve to a regular file.
///
/// Idempotent: the same resolved target always maps to the same token, so a
/// repeat call is a no-op that returns the same URL.
pub(crate) fn stage(local_root: &Path, base_dir: &Path, raw: &str) -> Option<String> {
    let resolved = resolve_link(base_dir, raw).ok()?;
    let token = token_for(&resolved);
    let dir = local_root.join("_local");
    fs::create_dir_all(&dir).ok()?;
    let link = dir.join(&token);
    match symlink(&resolved, &link) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
        Err(_) => return None,
    }
    Some(format!("_local/{token}"))
}

/// A content-addressed, filename-safe token for `resolved`: the blake3 hex of
/// its (canonical) path bytes, plus the original extension so the host's
/// extension-based MIME inference stays correct.
///
/// The extension is appended only when it is ASCII-alphanumeric. A raw
/// filesystem extension can contain URL-reserved characters (`#`, `?`, `%`);
/// splicing one into the returned URL would make the browser parse it as a
/// fragment/query and request a path that no longer matches the on-disk
/// symlink, silently breaking the image. Dropping such an extension falls back
/// to MIME sniffing rather than a broken request.
fn token_for(resolved: &Path) -> String {
    let hash = blake3::hash(resolved.as_os_str().as_bytes()).to_hex();
    match resolved.extension().and_then(|e| e.to_str()) {
        Some(ext) if ext.chars().all(|c| c.is_ascii_alphanumeric()) => format!("{hash}.{ext}"),
        _ => hash.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn token_is_stable_and_keeps_extension() {
        let p = Path::new("/tmp/whatever/a.PNG");
        assert_eq!(token_for(p), token_for(p));
        assert!(token_for(p).ends_with(".PNG"));
    }

    #[test]
    fn token_differs_for_different_paths() {
        assert_ne!(
            token_for(Path::new("/a/x.png")),
            token_for(Path::new("/b/x.png"))
        );
    }

    #[test]
    fn token_drops_url_unsafe_extension() {
        let t = token_for(Path::new("/x/photo.png#2"));
        assert!(!t.contains('#'));
        assert!(!t.contains('.'));
    }

    #[test]
    fn stage_creates_symlink_to_resolved_target_and_is_idempotent() {
        let base = tempfile::tempdir().unwrap();
        let img = base.path().join("pic.png");
        fs::write(&img, b"\x89PNG\r\n").unwrap();
        let root = tempfile::tempdir().unwrap();

        let url1 = stage(root.path(), base.path(), "pic.png").unwrap();
        assert!(url1.starts_with("_local/"));
        assert!(url1.ends_with(".png"));

        let link = root.path().join(&url1);
        assert!(link.exists());
        assert_eq!(
            fs::read_link(&link).unwrap(),
            fs::canonicalize(&img).unwrap()
        );

        assert_eq!(stage(root.path(), base.path(), "pic.png").unwrap(), url1);
    }

    #[test]
    fn stage_relative_resolves_against_base_dir() {
        let base = tempfile::tempdir().unwrap();
        let sub = base.path().join("img");
        fs::create_dir(&sub).unwrap();
        let img = sub.join("a.gif");
        fs::write(&img, b"x").unwrap();
        let root = tempfile::tempdir().unwrap();

        let url = stage(root.path(), base.path(), "img/a.gif").unwrap();
        let link = root.path().join(&url);
        assert_eq!(
            fs::read_link(&link).unwrap(),
            fs::canonicalize(&img).unwrap()
        );
    }

    #[test]
    fn stage_absolute_path_outside_base_dir() {
        let elsewhere = tempfile::tempdir().unwrap();
        let img = elsewhere.path().join("far.jpg");
        fs::write(&img, b"x").unwrap();
        let base = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();

        let url = stage(root.path(), base.path(), img.to_str().unwrap()).unwrap();
        assert!(url.ends_with(".jpg"));
    }

    #[test]
    fn stage_missing_file_returns_none() {
        let base = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        assert!(stage(root.path(), base.path(), "nope.png").is_none());
    }
}
