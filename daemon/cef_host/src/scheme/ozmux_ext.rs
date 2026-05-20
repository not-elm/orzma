//! `ozmux-ext://` scheme handler factory + non-blocking `ResourceHandler`.
//!
//! Maps `ozmux-ext://<extension-name>/<path>` to a file under the registered
//! extension's `launch_dir`. The factory resolves the file path against the
//! canonicalized `launch_dir` from `ExtensionRegistry` and returns a
//! `ResourceHandler` that streams the file in `read()` chunks. Path traversal
//! is rejected by re-canonicalising the resolved path and verifying it stays
//! under `launch_dir`.

use cef::rc::Rc;
use cef::{
    Callback, CefString, ImplRequest, ImplResourceHandler, ImplResponse, ImplSchemeHandlerFactory,
    Request, ResourceHandler, ResourceReadCallback, Response, SchemeHandlerFactory,
    WrapResourceHandler, WrapSchemeHandlerFactory, wrap_resource_handler,
    wrap_scheme_handler_factory,
};
use ozmux_extension::registry::ExtensionRegistry;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Builds an `OzmuxExtSchemeHandlerFactory` wired to the registry that supplies
/// extension `launch_dir`s. Returned as the opaque `SchemeHandlerFactory` so
/// callers in `pool.rs` can pass it straight into
/// `RequestContext::register_scheme_handler_factory`.
///
/// `ExtensionRegistry` is internally `Arc<RwLock<…>>` so cloning the value
/// passed in shares state with every other holder.
pub fn make_factory(extensions: ExtensionRegistry) -> SchemeHandlerFactory {
    OzmuxExtSchemeHandlerFactory::new(extensions)
}

/// Parses an `ozmux-ext://<host>/<path>` URL into `(host, path)`. Empty paths
/// default to `index.html`. Returns `None` for malformed input (wrong scheme
/// or empty host).
pub(crate) fn parse_ozmux_ext_url(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("ozmux-ext://")?;
    let (host, path) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx + 1..]),
        None => (rest, ""),
    };
    if host.is_empty() {
        return None;
    }
    let path = if path.is_empty() { "index.html" } else { path };
    Some((host.to_string(), path.to_string()))
}

/// Resolves `rel` under `base` and rejects any path that escapes `base` via
/// `..` traversal or symlinks. Returns `None` if the resolved file does not
/// exist or canonicalises outside `base`.
///
/// `base` MUST already be canonicalised by the caller — this function only
/// canonicalises the candidate child so the `starts_with` check is reliable.
pub(crate) fn resolve_under_base(base: &Path, rel: &str) -> Option<PathBuf> {
    let candidate = base.join(rel).canonicalize().ok()?;
    if candidate.starts_with(base) {
        Some(candidate)
    } else {
        None
    }
}

wrap_scheme_handler_factory! {
    struct OzmuxExtSchemeHandlerFactory {
        extensions: ExtensionRegistry,
    }

    impl SchemeHandlerFactory {
        fn create(
            &self,
            _browser: Option<&mut cef::Browser>,
            _frame: Option<&mut cef::Frame>,
            _scheme_name: Option<&CefString>,
            request: Option<&mut Request>,
        ) -> Option<ResourceHandler> {
            let request = request?;
            let url = CefString::from(&request.url()).to_string();
            let resolved = resolve_for_url(&self.extensions, &url);
            match resolved {
                Some((path, mime)) => Some(OzmuxExtResourceHandler::new(
                    Arc::new(Mutex::new(None)),
                    path,
                    mime,
                )),
                None => Some(not_found_handler()),
            }
        }
    }
}

wrap_resource_handler! {
    struct OzmuxExtResourceHandler {
        state: Arc<Mutex<Option<OpenFile>>>,
        path: PathBuf,
        mime: String,
    }

    impl ResourceHandler {
        fn open(
            &self,
            _request: Option<&mut Request>,
            handle_request: Option<&mut i32>,
            _callback: Option<&mut Callback>,
        ) -> i32 {
            // CEF documents `open`/`read` as called on a dedicated worker
            // thread (NOT UI / NOT IO), so a blocking `File::open` is safe.
            let opened = File::open(&self.path)
                .and_then(|f| f.metadata().map(|m| (f, m.len())))
                .ok();
            let success = opened.is_some();
            if let Ok(mut guard) = self.state.lock() {
                *guard = opened.map(|(file, size)| OpenFile { file, size });
            }
            if let Some(handle_request) = handle_request {
                *handle_request = 1;
            }
            if success { 1 } else { 0 }
        }

        fn response_headers(
            &self,
            response: Option<&mut Response>,
            response_length: Option<&mut i64>,
            _redirect_url: Option<&mut CefString>,
        ) {
            let Some(response) = response else { return };
            let size = self
                .state
                .lock()
                .ok()
                .and_then(|g| g.as_ref().map(|o| o.size as i64));
            match size {
                Some(len) => {
                    response.set_status(200);
                    response.set_mime_type(Some(&CefString::from(self.mime.as_str())));
                    // Disable caching so extension dev hot-reload picks up
                    // edits without a forced refresh.
                    response.set_header_by_name(
                        Some(&CefString::from("Cache-Control")),
                        Some(&CefString::from("no-store")),
                        1,
                    );
                    if let Some(out) = response_length {
                        *out = len;
                    }
                }
                None => {
                    response.set_status(404);
                    response.set_mime_type(Some(&CefString::from("text/plain")));
                    if let Some(out) = response_length {
                        *out = 0;
                    }
                }
            }
        }

        #[allow(clippy::not_unsafe_ptr_arg_deref)]
        fn read(
            &self,
            data_out: *mut u8,
            bytes_to_read: i32,
            bytes_read: Option<&mut i32>,
            _callback: Option<&mut ResourceReadCallback>,
        ) -> i32 {
            let Some(bytes_read) = bytes_read else {
                return 0;
            };
            if bytes_to_read <= 0 {
                *bytes_read = 0;
                return 0;
            }
            let Ok(mut guard) = self.state.lock() else {
                *bytes_read = 0;
                return 0;
            };
            let Some(open) = guard.as_mut() else {
                *bytes_read = 0;
                return 0;
            };
            // SAFETY: `data_out` is a CEF-owned buffer of at least
            // `bytes_to_read` bytes for the duration of this call; constructing
            // a temporary `&mut [u8]` from it is sound and lets us delegate to
            // `Read::read`. The slice does not outlive the call.
            let buf =
                unsafe { std::slice::from_raw_parts_mut(data_out, bytes_to_read as usize) };
            match open.file.read(buf) {
                Ok(0) => {
                    *bytes_read = 0;
                    0
                }
                Ok(n) => {
                    *bytes_read = n as i32;
                    1
                }
                Err(e) => {
                    tracing::error!(path = %self.path.display(), error = %e, "ozmux-ext read failed");
                    *bytes_read = 0;
                    0
                }
            }
        }

        fn cancel(&self) {
            if let Ok(mut guard) = self.state.lock() {
                *guard = None;
            }
        }
    }
}

struct OpenFile {
    file: File,
    size: u64,
}

fn resolve_for_url(extensions: &ExtensionRegistry, url: &str) -> Option<(PathBuf, String)> {
    let (host, path) = parse_ozmux_ext_url(url)?;
    let info = extensions.get(&host)?;
    let resolved = resolve_under_base(&info.launch_dir, &path)?;
    let mime = mime_guess::from_path(&resolved)
        .first_or_octet_stream()
        .essence_str()
        .to_string();
    Some((resolved, mime))
}

fn not_found_handler() -> ResourceHandler {
    OzmuxExtResourceHandler::new(
        Arc::new(Mutex::new(None)),
        PathBuf::new(),
        "text/plain".to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn parses_extension_and_path() {
        let (host, path) = parse_ozmux_ext_url("ozmux-ext://memo/index.html").unwrap();
        assert_eq!(host, "memo");
        assert_eq!(path, "index.html");
    }

    #[test]
    fn empty_path_defaults_to_index_html() {
        let (host, path) = parse_ozmux_ext_url("ozmux-ext://memo/").unwrap();
        assert_eq!(host, "memo");
        assert_eq!(path, "index.html");

        let (host, path) = parse_ozmux_ext_url("ozmux-ext://memo").unwrap();
        assert_eq!(host, "memo");
        assert_eq!(path, "index.html");
    }

    #[test]
    fn rejects_empty_host() {
        assert!(parse_ozmux_ext_url("ozmux-ext:///foo").is_none());
    }

    #[test]
    fn rejects_wrong_scheme() {
        assert!(parse_ozmux_ext_url("https://memo/index.html").is_none());
    }

    #[test]
    fn parses_nested_path() {
        let (host, path) = parse_ozmux_ext_url("ozmux-ext://memo/assets/app.js").unwrap();
        assert_eq!(host, "memo");
        assert_eq!(path, "assets/app.js");
    }

    #[test]
    fn resolves_normal_path() {
        let tmp = tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let file = base.join("a.txt");
        std::fs::write(&file, b"hi").unwrap();

        let resolved = resolve_under_base(&base, "a.txt").expect("resolves");
        assert_eq!(resolved, file.canonicalize().unwrap());
    }

    #[test]
    fn rejects_traversal() {
        let tmp = tempdir().unwrap();
        let outer = tmp.path().canonicalize().unwrap();
        let base = outer.join("inside");
        std::fs::create_dir(&base).unwrap();
        let escape = outer.join("escape.txt");
        std::fs::write(&escape, b"escape").unwrap();

        assert!(resolve_under_base(&base, "../escape.txt").is_none());
    }

    #[test]
    fn rejects_missing_file() {
        let tmp = tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        assert!(resolve_under_base(&base, "nope.html").is_none());
    }
}
