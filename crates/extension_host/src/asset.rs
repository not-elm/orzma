//! Static-asset resolution for the in-process custom-scheme asset path
//! (`ozmux-dyn://`): percent-decode a webview-supplied request path, reject
//! traversal, read the file under the registered asset root, and infer a bare
//! MIME type. Pure (no `cef` dependency) so it is unit-testable on its own.

use std::path::Path;

/// The outcome of resolving and reading one static-asset request.
#[derive(Debug, PartialEq, Eq)]
pub enum AssetOutcome {
    /// The file was read; carries a bare MIME type and the bytes.
    Ok {
        /// Bare MIME type (no parameters), e.g. `"text/html"`.
        content_type: String,
        /// Raw file bytes.
        body: Vec<u8>,
    },
    /// No such file under the asset root.
    NotFound,
    /// The request path was malformed or attempted to escape the asset root.
    Forbidden,
    /// The file exceeded the internal `MAX_ASSET_LEN` cap.
    TooLarge,
}

/// Resolves `raw_path` (a percent-encoded, slash-separated relative URL path)
/// under `root` and reads the file, returning a bare MIME type.
///
/// `raw_path` is webview-controlled, so it is decoded exactly once and then
/// rejected unless every component is a normal path segment — `..`, `.`, an
/// absolute path, or a non-UTF-8 / malformed percent escape all yield
/// [`AssetOutcome::Forbidden`]. This is the trust boundary that keeps a mounted
/// page from reading files outside its extension directory.
pub fn serve_static_asset(root: &Path, raw_path: &str) -> AssetOutcome {
    let Some(decoded) = percent_decode(raw_path) else {
        return AssetOutcome::Forbidden;
    };
    let rel = Path::new(&decoded);
    if !is_safe_rel_path(rel) {
        return AssetOutcome::Forbidden;
    }
    let full = root.join(rel);
    let meta = match std::fs::metadata(&full) {
        Ok(m) => m,
        Err(_) => return AssetOutcome::NotFound,
    };
    if !meta.is_file() {
        return AssetOutcome::NotFound;
    }
    if exceeds_limit(meta.len()) {
        return AssetOutcome::TooLarge;
    }
    match std::fs::read(&full) {
        Ok(body) => AssetOutcome::Ok {
            content_type: mime_for_path(&full).to_string(),
            body,
        },
        Err(_) => AssetOutcome::NotFound,
    }
}

/// Upper bound on a single static asset (64 MiB): a larger file would buffer
/// wholesale into the render process.
const MAX_ASSET_LEN: u64 = 64 * 1024 * 1024;

fn exceeds_limit(len: u64) -> bool {
    len > MAX_ASSET_LEN
}

/// Decodes `%XX` escapes once. Returns `None` on a truncated/invalid escape or
/// when the decoded bytes are not valid UTF-8. Does not treat `+` as space
/// (that is a query-string rule, not a path rule).
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
// TODO: lexical check only — a symlink inside the extension dir is still followed by std::fs::read; add a canonicalize + prefix check if extension-dir contents ever become untrusted (Phase 1 trusts them).
pub fn is_safe_rel_path(p: &Path) -> bool {
    !p.as_os_str().is_empty()
        && p.components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
}

/// Maps a file extension to a bare MIME type for the asset set a Phase 1
/// extension ships. Unknown extensions fall back to `application/octet-stream`.
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

    #[test]
    fn percent_decode_basic_and_escapes() {
        assert_eq!(percent_decode("index.html").as_deref(), Some("index.html"));
        assert_eq!(percent_decode("a%2Fb").as_deref(), Some("a/b"));
        assert_eq!(percent_decode("%2e%2e").as_deref(), Some(".."));
    }

    #[test]
    fn percent_decode_rejects_malformed() {
        assert_eq!(percent_decode("%2"), None);
        assert_eq!(percent_decode("%zz"), None);
        assert_eq!(percent_decode("%ff%fe"), None);
    }

    #[test]
    fn is_safe_rel_path_rejects_traversal_and_absolute() {
        assert!(is_safe_rel_path(Path::new("index.html")));
        assert!(is_safe_rel_path(Path::new("sub/app.js")));
        assert!(!is_safe_rel_path(Path::new("../escape")));
        assert!(!is_safe_rel_path(Path::new("a/../b")));
        assert!(!is_safe_rel_path(Path::new("/etc/passwd")));
        assert!(!is_safe_rel_path(Path::new("")));
    }

    #[test]
    fn mime_for_common_extensions() {
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

    #[test]
    fn exceeds_limit_at_boundary() {
        assert!(!exceeds_limit(MAX_ASSET_LEN));
        assert!(exceeds_limit(MAX_ASSET_LEN + 1));
    }

    #[test]
    fn serves_a_real_file_with_inferred_mime() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("index.html"), b"<h1>hi</h1>").unwrap();
        let out = serve_static_asset(dir.path(), "index.html");
        assert_eq!(
            out,
            AssetOutcome::Ok {
                content_type: "text/html".into(),
                body: b"<h1>hi</h1>".to_vec(),
            }
        );
    }

    #[test]
    fn serves_nested_file() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("assets")).unwrap();
        fs::write(dir.path().join("assets/app.js"), b"x").unwrap();
        let out = serve_static_asset(dir.path(), "assets/app.js");
        assert!(matches!(
            out,
            AssetOutcome::Ok { content_type, .. } if content_type == "text/javascript"
        ));
    }

    #[test]
    fn missing_file_is_not_found() {
        let dir = tempdir().unwrap();
        assert_eq!(
            serve_static_asset(dir.path(), "nope.html"),
            AssetOutcome::NotFound
        );
    }

    #[test]
    fn directory_path_is_not_found() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("sub")).unwrap();
        assert_eq!(
            serve_static_asset(dir.path(), "sub"),
            AssetOutcome::NotFound
        );
    }

    #[test]
    fn literal_traversal_is_forbidden_and_does_not_read_outside_root() {
        let parent = tempdir().unwrap();
        fs::write(parent.path().join("secret.txt"), b"top secret").unwrap();
        let root = parent.path().join("ext");
        fs::create_dir_all(&root).unwrap();
        assert_eq!(
            serve_static_asset(&root, "../secret.txt"),
            AssetOutcome::Forbidden
        );
    }

    #[test]
    fn percent_encoded_traversal_is_forbidden() {
        let parent = tempdir().unwrap();
        fs::write(parent.path().join("secret.txt"), b"top secret").unwrap();
        let root = parent.path().join("ext");
        fs::create_dir_all(&root).unwrap();
        assert_eq!(
            serve_static_asset(&root, "%2e%2e%2fsecret.txt"),
            AssetOutcome::Forbidden
        );
    }
}
