//! Stages local image files referenced by a Markdown document under the served
//! asset root's `_local/` directory, so the webview can load them as
//! `orzma://<handle>/_local/<token>.<ext>` subresources. Each entry is a
//! symlink to the source file, or on Windows a copy when the user may not
//! create symlinks.

use crate::document::resolve_link;
use std::fs;
use std::io::{self, ErrorKind};
#[cfg(unix)]
use std::os::unix::fs::symlink;
#[cfg(windows)]
use std::os::windows::fs::symlink_file;
use std::path::Path;

/// Stages the local file `raw` (resolved against `base_dir`) under
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
    match link_or_copy(&resolved, &link) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
        Err(_) => return None,
    }
    Some(format!("_local/{token}"))
}

/// Creates `link` as a symlink to `target`.
#[cfg(unix)]
fn link_or_copy(target: &Path, link: &Path) -> io::Result<()> {
    symlink(target, link)
}

/// Creates `link` as a symlink to `target`, or as a copy of it when the
/// symlink cannot be created (a file symlink needs Developer Mode or
/// administrator rights). An existing `link` is reported as `AlreadyExists`
/// rather than overwritten.
#[cfg(windows)]
fn link_or_copy(target: &Path, link: &Path) -> io::Result<()> {
    match symlink_file(target, link) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::AlreadyExists => Err(e),
        Err(_) => {
            if link.exists() {
                return Err(io::Error::from(ErrorKind::AlreadyExists));
            }
            fs::copy(target, link).map(|_| ())
        }
    }
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
    let hash = blake3::hash(resolved.as_os_str().as_encoded_bytes()).to_hex();
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

    /// Asserts that staging serves the resolved image's bytes under a
    /// `_local/` URL and that a repeat call returns the same URL.
    ///
    /// Case: a Markdown file references `pic.png` next to it, and a later
    /// re-render references the same image again.
    #[test]
    fn stage_serves_the_resolved_target_and_is_idempotent() {
        let base = tempfile::tempdir().unwrap();
        let img = base.path().join("pic.png");
        fs::write(&img, b"\x89PNG\r\n").unwrap();
        let root = tempfile::tempdir().unwrap();

        let url1 = stage(root.path(), base.path(), "pic.png").unwrap();
        assert!(url1.starts_with("_local/"));
        assert!(url1.ends_with(".png"));

        let staged = root.path().join(&url1);
        assert_eq!(fs::read(&staged).unwrap(), fs::read(&img).unwrap());

        assert_eq!(stage(root.path(), base.path(), "pic.png").unwrap(), url1);
    }

    /// Asserts that on Unix the staged entry is a symlink to the canonical
    /// target rather than a copy.
    ///
    /// Case: the user edits an image in place after the document has been
    /// rendered once, and the next render should show the new bytes.
    #[cfg(unix)]
    #[test]
    fn stage_symlinks_to_the_canonical_target_on_unix() {
        let base = tempfile::tempdir().unwrap();
        let img = base.path().join("pic.png");
        fs::write(&img, b"x").unwrap();
        let root = tempfile::tempdir().unwrap();

        let url = stage(root.path(), base.path(), "pic.png").unwrap();
        assert_eq!(
            fs::read_link(root.path().join(&url)).unwrap(),
            fs::canonicalize(&img).unwrap()
        );
    }

    /// Asserts that on Windows the staged entry serves the target's bytes
    /// whether or not the user may create symlinks.
    ///
    /// Case: orzmd runs on a Windows machine without Developer Mode, where a
    /// file symlink needs administrator rights.
    #[cfg(windows)]
    #[test]
    fn stage_serves_the_target_bytes_without_symlink_rights_on_windows() {
        let base = tempfile::tempdir().unwrap();
        let img = base.path().join("pic.png");
        fs::write(&img, b"x").unwrap();
        let root = tempfile::tempdir().unwrap();

        let url = stage(root.path(), base.path(), "pic.png").unwrap();
        let staged = root.path().join(&url);
        assert!(staged.is_file());
        assert_eq!(fs::read(&staged).unwrap(), b"x");
    }

    /// Asserts that a relative link is resolved against the document's base
    /// directory, not the process working directory.
    ///
    /// Case: `img/a.gif` is referenced from a document opened by absolute
    /// path from a different working directory.
    #[test]
    fn stage_relative_resolves_against_base_dir() {
        let base = tempfile::tempdir().unwrap();
        let sub = base.path().join("img");
        fs::create_dir(&sub).unwrap();
        let img = sub.join("a.gif");
        fs::write(&img, b"x").unwrap();
        let root = tempfile::tempdir().unwrap();

        let url = stage(root.path(), base.path(), "img/a.gif").unwrap();
        assert_eq!(
            fs::read(root.path().join(&url)).unwrap(),
            fs::read(&img).unwrap()
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
