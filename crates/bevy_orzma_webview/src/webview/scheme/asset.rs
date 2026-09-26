//! Static-asset resolution for the `orzma://` custom scheme: percent-decode a
//! webview-supplied request path, reject traversal, read the file under the
//! registered asset root, and infer a bare MIME type.

use crate::error::{WebviewError, WebviewResult};
use std::path::{Component, Path};

/// One static asset read from disk.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct StaticAsset {
    content_type: &'static str,
    body: Vec<u8>,
}

impl StaticAsset {
    /// Resolves `raw_path` (a percent-encoded, slash-separated relative URL
    /// path) under `root` and reads the file.
    ///
    /// `raw_path` is decoded exactly once and refused unless every component
    /// is a normal path segment, so no request reads outside `root`.
    ///
    /// # Errors
    ///
    /// Returns [`WebviewError::AssetForbidden`] for a malformed or non-UTF-8
    /// escape, `..`, `.`, or an absolute path; [`WebviewError::AssetNotFound`]
    /// when no readable file exists there; and [`WebviewError::AssetTooLarge`]
    /// for a file larger than 64 MiB.
    pub fn read(root: &Path, raw_path: &str) -> WebviewResult<Self> {
        let decoded = percent_decode(raw_path).ok_or(WebviewError::AssetForbidden)?;
        let rel = Path::new(&decoded);
        if !is_safe_rel_path(rel) {
            return Err(WebviewError::AssetForbidden);
        }
        let full = root.join(rel);
        let meta = std::fs::metadata(&full).map_err(|_| WebviewError::AssetNotFound)?;
        if !meta.is_file() {
            return Err(WebviewError::AssetNotFound);
        }
        if exceeds_limit(meta.len()) {
            return Err(WebviewError::AssetTooLarge);
        }
        let body = std::fs::read(&full).map_err(|_| WebviewError::AssetNotFound)?;
        Ok(Self {
            content_type: mime_for_path(&full),
            body,
        })
    }

    /// The bare MIME type, without parameters, such as `"text/html"`.
    pub fn content_type(&self) -> &'static str {
        self.content_type
    }

    /// The file's bytes.
    pub fn into_body(self) -> Vec<u8> {
        self.body
    }
}

/// Upper bound on a single static asset (64 MiB).
const MAX_ASSET_LEN: u64 = 64 * 1024 * 1024;

fn exceeds_limit(len: u64) -> bool {
    len > MAX_ASSET_LEN
}

/// Decodes `%XX` escapes once. Returns `None` on a truncated/invalid escape or
/// when the decoded bytes are not valid UTF-8. `+` is not treated as a space.
fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return None;
            }
            let hi = (bytes[i + 1] as char).to_digit(16)?;
            let lo = (bytes[i + 2] as char).to_digit(16)?;
            out.push((hi * 16 + lo) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// True when `p` is a non-empty relative path made only of normal components
/// (no `..`, no `.`, no leading `/`, no Windows prefix).
// TODO: lexical check only — a symlink inside the webview dir is still followed by std::fs::read; add a canonicalize + prefix check if webview-dir contents ever become untrusted (Phase 1 trusts them).
fn is_safe_rel_path(p: &Path) -> bool {
    !p.as_os_str().is_empty() && p.components().all(|c| matches!(c, Component::Normal(_)))
}

/// Maps a file extension to a bare MIME type. Unknown extensions fall back to
/// `application/octet-stream`.
fn mime_for_path(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("html" | "htm") => "text/html",
        Some("js" | "mjs") => "text/javascript",
        Some("css") => "text/css",
        Some("json") => "application/json",
        Some("wasm") => "application/wasm",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("ico") => "image/x-icon",
        Some("map" | "txt") => "text/plain",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    /// Asserts that escapes are decoded exactly once, so an encoded `..`
    /// decodes to `..` rather than to a path segment.
    ///
    /// Case: a page requests a file whose name holds an encoded slash.
    #[test]
    fn percent_decode_decodes_escapes_once() {
        assert_eq!(percent_decode("index.html").as_deref(), Some("index.html"));
        assert_eq!(percent_decode("a%2Fb").as_deref(), Some("a/b"));
        assert_eq!(percent_decode("%2e%2e").as_deref(), Some(".."));
    }

    /// Asserts that a truncated escape, a non-hex escape, and bytes that are
    /// not UTF-8 all fail to decode.
    ///
    /// Case: a hostile page crafts request paths with broken escapes.
    #[test]
    fn percent_decode_rejects_malformed_escapes() {
        assert_eq!(percent_decode("%2"), None);
        assert_eq!(percent_decode("%zz"), None);
        assert_eq!(percent_decode("%ff%fe"), None);
    }

    /// Asserts that only non-empty relative paths of normal components pass
    /// the path check.
    ///
    /// Case: a page requests `../escape`, `a/../b`, `/etc/passwd`, and an
    /// empty path next to two legitimate files.
    #[test]
    fn is_safe_rel_path_rejects_traversal_and_absolute_paths() {
        assert!(is_safe_rel_path(Path::new("index.html")));
        assert!(is_safe_rel_path(Path::new("sub/app.js")));
        assert!(!is_safe_rel_path(Path::new("../escape")));
        assert!(!is_safe_rel_path(Path::new("a/../b")));
        assert!(!is_safe_rel_path(Path::new("/etc/passwd")));
        assert!(!is_safe_rel_path(Path::new("")));
    }

    /// Asserts that common web extensions map to their MIME types, case
    /// insensitively, and that an unknown extension falls back to
    /// `application/octet-stream`.
    ///
    /// Case: a directory registration serves HTML, scripts, styles, wasm,
    /// and an uppercase SVG.
    #[test]
    fn mime_types_follow_common_extensions() {
        assert_eq!(mime_for_path(Path::new("index.html")), "text/html");
        assert_eq!(mime_for_path(Path::new("app.mjs")), "text/javascript");
        assert_eq!(mime_for_path(Path::new("style.css")), "text/css");
        assert_eq!(mime_for_path(Path::new("bin.wasm")), "application/wasm");
        assert_eq!(mime_for_path(Path::new("logo.SVG")), "image/svg+xml");
        assert_eq!(
            mime_for_path(Path::new("noext")),
            "application/octet-stream"
        );
    }

    /// Asserts that the size cap admits exactly 64 MiB and refuses one byte
    /// more.
    ///
    /// Case: a page loads a bundle that sits right at the cap.
    #[test]
    fn the_size_cap_admits_exactly_64_mib() {
        assert!(!exceeds_limit(MAX_ASSET_LEN));
        assert!(exceeds_limit(MAX_ASSET_LEN + 1));
    }

    /// Asserts that a file under the root is read with the MIME type its
    /// extension implies.
    ///
    /// Case: a page registered from a directory loads its `index.html`.
    #[test]
    fn a_file_is_served_with_the_mime_type_its_extension_implies() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("index.html"), b"<h1>hi</h1>").unwrap();
        assert_eq!(
            StaticAsset::read(dir.path(), "index.html"),
            Ok(StaticAsset {
                content_type: "text/html",
                body: b"<h1>hi</h1>".to_vec(),
            })
        );
    }

    /// Asserts that a file in a subdirectory of the root is served.
    ///
    /// Case: the page's `index.html` loads `assets/app.js`.
    #[test]
    fn a_nested_file_is_served() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("assets")).unwrap();
        fs::write(dir.path().join("assets/app.js"), b"x").unwrap();
        let asset = StaticAsset::read(dir.path(), "assets/app.js").expect("the file is served");
        assert_eq!(asset.content_type(), "text/javascript");
    }

    /// Asserts that a path with no file behind it is not found.
    ///
    /// Case: a page requests a script its bundle does not ship.
    #[test]
    fn a_missing_file_is_not_found() {
        let dir = tempdir().unwrap();
        assert_eq!(
            StaticAsset::read(dir.path(), "nope.html"),
            Err(WebviewError::AssetNotFound)
        );
    }

    /// Asserts that a directory is not served as a file.
    ///
    /// Case: a page requests a folder of its bundle by name.
    #[test]
    fn a_directory_is_not_found() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("sub")).unwrap();
        assert_eq!(
            StaticAsset::read(dir.path(), "sub"),
            Err(WebviewError::AssetNotFound)
        );
    }

    /// Asserts that a literal `..` path is forbidden and reads nothing
    /// outside the root.
    ///
    /// Case: a hostile page asks for a secret file next to its bundle.
    #[test]
    fn a_literal_traversal_is_forbidden() {
        let parent = tempdir().unwrap();
        fs::write(parent.path().join("secret.txt"), b"top secret").unwrap();
        let root = parent.path().join("ext");
        fs::create_dir_all(&root).unwrap();
        assert_eq!(
            StaticAsset::read(&root, "../secret.txt"),
            Err(WebviewError::AssetForbidden)
        );
    }

    /// Asserts that a percent-encoded `..` path is forbidden too.
    ///
    /// Case: a hostile page encodes its traversal to slip past a naive
    /// check.
    #[test]
    fn a_percent_encoded_traversal_is_forbidden() {
        let parent = tempdir().unwrap();
        fs::write(parent.path().join("secret.txt"), b"top secret").unwrap();
        let root = parent.path().join("ext");
        fs::create_dir_all(&root).unwrap();
        assert_eq!(
            StaticAsset::read(&root, "%2e%2e%2fsecret.txt"),
            Err(WebviewError::AssetForbidden)
        );
    }
}
