# Phase 1 Step 3b-3: Rust-Direct Static Asset Serving Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make OSC-mounted extension webviews render their static assets by serving them directly from Rust in the `ozmux-ext://` scheme handler, instead of round-tripping through the Node host.

**Architecture:** Decision C from the spec (`docs/superpowers/specs/2026-06-11-phase1-single-host-process-design.md` §4④). The single shared dispatch map becomes one source-discriminating registry, `name → AssetSource` where `AssetSource = Static(PathBuf) | Legacy(ExtensionEndpoints)`. New-model extensions (discovered via `ozmux.toml`) register `Static(<dir>)`; the scheme handler resolves `dir.join(path)`, validates traversal, reads the file, and infers MIME by extension. Legacy command-extensions keep `Legacy(ExtensionEndpoints)` and the existing socket `fetch` (coexistence dual-path, removed in Step 5).

**Tech Stack:** Rust (Bevy 0.18 plugin wiring), `bevy_cef_core` custom scheme (behind the `cef` feature), `std::fs`. No new crates — percent-decode and MIME are small hand-rolled functions. TypeScript/Node host change (Task 3) via the existing `host/` esbuild bundle.

**Key facts (verified against current code):**
- The `ozmux-ext://` scheme handler `CefSchemeHandler::handle` runs on a CEF resource-handler worker thread (not the Bevy/IO/UI thread) and the current code already does a blocking socket `fetch` + `CefSchemeBody::Bytes` there, so a synchronous `std::fs::read` is legal and cheaper. (`crates/extension_host/src/scheme.rs:85-118`)
- `parse_url` (`scheme.rs:18-32`) does NOT strip `..`; the request path is webview-controlled, so a traversal check on the request path is mandatory. The build-time `is_safe_rel` (`host_descriptor.rs:91-98`) only guards manifest paths, not request paths.
- `DiscoveredExtension.dir` is the absolute extension directory — feed the registry from it directly. (`extension_discovery.rs:9-16`)
- Current dispatch map is `EndpointRegistry = Arc<RwLock<HashMap<String, ExtensionEndpoints>>>` (`host.rs:128-145`), read live per request by the handler (proven by the late-insert test, `scheme.rs:212-230`).

**Verification commands:**
- Crate tests: `cargo test -p ozmux_extension_host`
- Full build: `cargo build`
- Host (Node) tests: `pnpm -C host test`
- NOTE: a full `cargo test` has a pre-existing IME failure + a parallel-teardown SIGSEGV unrelated to this work; a green full run needs `--test-threads=1` plus the known skips. Per-crate `cargo test -p ozmux_extension_host` is unaffected — use it as the primary signal here.

---

## File Structure

**Create:**
- `crates/extension_host/src/asset.rs` — pure static-asset resolver: percent-decode, traversal validation, MIME-by-extension, bounded file read. No `cef` gating; fully unit-testable.

**Modify:**
- `crates/extension_host/src/lib.rs` — add `pub mod asset;`; swap the `host` re-exports (`EndpointRegistry` → `AssetSource`, `AssetSourceRegistry`).
- `crates/extension_host/src/host.rs` — add `AssetSource` enum + `AssetSourceRegistry`; remove `EndpointRegistry`. Keep `ExtensionEndpoints`, `fetch`, `FetchError`.
- `crates/extension_host/src/scheme.rs` — `resolve_request` returns `(AssetSource, &str)`; `handle` branches `Static` (call `serve_static_asset`) vs `Legacy` (existing `fetch`). Update `OzmuxExtScheme`/`custom_scheme` to `AssetSourceRegistry`.
- `src/extension_render.rs` — `cef_plugin(registry: AssetSourceRegistry)`.
- `src/main.rs` — construct `AssetSourceRegistry`; pass clones to `cef_plugin` and `ExtensionManagerPlugin::new`.
- `src/extension_manager.rs` — `ExtensionRegistry`/`ExtensionManagerPlugin` hold `AssetSourceRegistry`; legacy build path inserts `AssetSource::Legacy(...)`; `spawn_single_host` inserts `AssetSource::Static(dir)`; `publish_ready_endpoints` uses `legacy_endpoint`.
- `crates/extension_host/src/host_descriptor.rs` — (Task 3) drop `asset_root` from `ExtensionDescriptorJson`.
- `host/src/descriptors.ts`, `host/src/load.ts`, `assets/host.mjs` — (Task 3) drop `assetRoot` from the Node zod schema + rebuild bundle.

---

## Task 1: Pure static-asset resolver (`asset.rs`)

This module is self-contained and `cef`-independent — it does not break any existing build.

**Files:**
- Create: `crates/extension_host/src/asset.rs`
- Modify: `crates/extension_host/src/lib.rs`

- [ ] **Step 1: Create `asset.rs` with the resolver and its tests**

Create `crates/extension_host/src/asset.rs`:

```rust
//! Static-asset resolution for the Rust-direct asset path (spec §4④ decision C):
//! percent-decode a webview-supplied request path, reject traversal, read the
//! file under the extension's asset root, and infer a bare MIME type. Pure (no
//! `cef` dependency) so it is unit-testable on its own.

use std::path::Path;

/// Upper bound on a single static asset, mirroring `protocol::MAX_BODY_LEN`
/// (64 MiB): a larger file would buffer wholesale into the render process.
const MAX_ASSET_LEN: u64 = 64 * 1024 * 1024;

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
    /// The file exceeded [`MAX_ASSET_LEN`].
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
/// (no `..`, no `.`, no leading `/`, no Windows prefix). Mirrors
/// `host_descriptor::is_safe_rel`, applied here to the request path.
fn is_safe_rel_path(p: &Path) -> bool {
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
        assert_eq!(percent_decode("%ff%fe"), None); // invalid UTF-8
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
        // %2e%2e%2f == "../"
        assert_eq!(
            serve_static_asset(&root, "%2e%2e%2fsecret.txt"),
            AssetOutcome::Forbidden
        );
    }
}
```

- [ ] **Step 2: Register the module**

Modify `crates/extension_host/src/lib.rs` — add `pub mod asset;` in the module block (keep alphabetical-ish ordering, right after the `//!` doc and existing `pub mod` lines; place it before `pub mod bridge;`):

```rust
pub mod asset;
pub mod bridge;
```

- [ ] **Step 3: Run the tests (expect PASS — TDD here is test+impl together since the module is new)**

Run: `cargo test -p ozmux_extension_host asset::`
Expected: all `asset::tests::*` PASS.

- [ ] **Step 4: Lint + format**

Run: `cargo clippy -p ozmux_extension_host --all-targets && cargo fmt`
Expected: no warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/extension_host/src/asset.rs crates/extension_host/src/lib.rs
git commit -m "feat(extension_host): add pure static-asset resolver (decision C)"
```

---

## Task 2: Source-discriminating registry + Rust-direct dispatch + wiring

Atomic refactor: swap `EndpointRegistry` for `AssetSourceRegistry` across the crate and the binary so the tree stays green at the end. Do every sub-step before building.

**Files:**
- Modify: `crates/extension_host/src/host.rs`
- Modify: `crates/extension_host/src/lib.rs`
- Modify: `crates/extension_host/src/scheme.rs`
- Modify: `src/extension_render.rs`
- Modify: `src/main.rs`
- Modify: `src/extension_manager.rs`

- [ ] **Step 1: Add `AssetSource` + `AssetSourceRegistry` to `host.rs`, remove `EndpointRegistry`**

In `crates/extension_host/src/host.rs`, replace the `EndpointRegistry` definition (the block at `host.rs:128-145`, the doc comment through the `impl EndpointRegistry { ... }`) with the following. Keep `ExtensionEndpoints` (above it) and `fetch`/`FetchError` (below it) exactly as they are.

```rust
/// The asset source for one extension name, dispatched by the `ozmux-ext://`
/// scheme handler. New-model extensions serve static files directly from a
/// directory; legacy command-extensions fetch over their per-extension socket.
#[derive(Clone)]
pub enum AssetSource {
    /// Serve static files directly from this extension directory (decision C).
    Static(PathBuf),
    /// Fetch assets over the legacy per-extension socket endpoint.
    Legacy(ExtensionEndpoints),
}

/// A shared, interior-mutable map of extension name → its [`AssetSource`]. Built
/// before extensions launch (the CEF scheme handler is constructed at
/// `CefPlugin::build()`) and populated as each extension is discovered/becomes
/// ready, so the handler reads names registered after its own construction.
///
/// # Invariants
/// A `Legacy` entry's [`ExtensionEndpoints`] is the same handle the manager
/// publishes the live socket path into on readiness; `Static` entries are fixed
/// at discovery time.
#[derive(Clone, Default)]
pub struct AssetSourceRegistry(Arc<RwLock<HashMap<String, AssetSource>>>);

impl AssetSourceRegistry {
    /// Returns (cloning) the asset source for `name`, if registered.
    pub fn get(&self, name: &str) -> Option<AssetSource> {
        self.0.read().unwrap().get(name).cloned()
    }

    /// Inserts/replaces the asset source for `name`.
    pub fn insert(&self, name: impl Into<String>, source: AssetSource) {
        self.0.write().unwrap().insert(name.into(), source);
    }

    /// Returns the legacy endpoint handle for `name`, or `None` when the name is
    /// unregistered or is a `Static` source. Used to publish the live socket
    /// path on readiness.
    pub fn legacy_endpoint(&self, name: &str) -> Option<ExtensionEndpoints> {
        match self.0.read().unwrap().get(name) {
            Some(AssetSource::Legacy(ep)) => Some(ep.clone()),
            _ => None,
        }
    }
}
```

- [ ] **Step 2: Add a registry unit test in `host.rs`**

In the `#[cfg(test)] mod tests` block of `host.rs`, add:

```rust
    #[test]
    fn asset_registry_distinguishes_static_and_legacy() {
        use std::path::PathBuf;
        let reg = AssetSourceRegistry::default();
        reg.insert("memo", AssetSource::Static(PathBuf::from("/abs/memo")));
        reg.insert("md", AssetSource::Legacy(ExtensionEndpoints::default()));

        assert!(matches!(reg.get("memo"), Some(AssetSource::Static(p)) if p == PathBuf::from("/abs/memo")));
        assert!(matches!(reg.get("md"), Some(AssetSource::Legacy(_))));
        assert!(reg.get("ghost").is_none());

        // legacy_endpoint returns a handle only for Legacy entries.
        assert!(reg.legacy_endpoint("md").is_some());
        assert!(reg.legacy_endpoint("memo").is_none());
        assert!(reg.legacy_endpoint("ghost").is_none());
    }
```

- [ ] **Step 3: Update crate re-exports in `lib.rs`**

In `crates/extension_host/src/lib.rs`, the `host` types are reached via `pub mod host;` (no explicit `pub use` for `EndpointRegistry`), so no `pub use` edit is needed. Confirm there is no `EndpointRegistry` in any `pub use` line:

Run: `grep -n EndpointRegistry crates/extension_host/src/lib.rs`
Expected: no output.

- [ ] **Step 4: Rewrite `scheme.rs` dispatch for the two sources**

In `crates/extension_host/src/scheme.rs`:

(a) Replace the import block at the top (`scheme.rs:4-12`) with:

```rust
use crate::host::{AssetSource, AssetSourceRegistry};
#[cfg(feature = "cef")]
use crate::asset::{AssetOutcome, serve_static_asset};
#[cfg(feature = "cef")]
use crate::host::{FetchError, fetch};
#[cfg(feature = "cef")]
use bevy_cef_core::prelude::{
    CefCustomScheme, CefSchemeBody, CefSchemeHandler, CefSchemeOptions, CefSchemeRequest,
    CefSchemeResponse,
};
#[cfg(feature = "cef")]
use std::sync::Arc;
```

(b) Replace `OzmuxExtScheme` + its `impl` (the `registry: EndpointRegistry` struct and `new`, `scheme.rs:58-69`) with:

```rust
/// Serves `ozmux-ext://<name>/<path>` for every registered extension by
/// dispatching on `<name>` through a shared asset-source registry.
#[cfg(feature = "cef")]
pub struct OzmuxExtScheme {
    registry: AssetSourceRegistry,
}

#[cfg(feature = "cef")]
impl OzmuxExtScheme {
    /// Builds a handler bound to the shared asset-source registry.
    pub fn new(registry: AssetSourceRegistry) -> Self {
        Self { registry }
    }
}
```

(c) Replace `resolve_request` (`scheme.rs:71-82`, currently `#[cfg(feature = "cef")]` and returning `(ExtensionEndpoints, &str)`) with a non-gated version returning the source:

```rust
/// Resolves the [`AssetSource`] for an `ozmux-ext://<name>/<path>` URL via the
/// registry. Returns `Ok((source, path))` to serve, or `Err(status)` for a
/// direct error response (404 unknown/unparseable name).
#[cfg_attr(not(feature = "cef"), allow(dead_code))]
fn resolve_request<'a>(
    registry: &AssetSourceRegistry,
    url: &'a str,
) -> Result<(AssetSource, &'a str), u16> {
    let (name, path) = parse_url(url).ok_or(404u16)?;
    let source = registry.get(name).ok_or(404u16)?;
    Ok((source, path))
}
```

(d) Replace the `impl CefSchemeHandler for OzmuxExtScheme` `handle` body (`scheme.rs:85-118`) with:

```rust
#[cfg(feature = "cef")]
impl CefSchemeHandler for OzmuxExtScheme {
    fn handle(&self, request: &CefSchemeRequest) -> CefSchemeResponse {
        let (source, path) = match resolve_request(&self.registry, &request.url) {
            Ok(resolved) => resolved,
            Err(404) => return CefSchemeResponse::not_found(),
            Err(status) => return status_text(status, "extension dispatch failed"),
        };
        match source {
            AssetSource::Static(root) => match serve_static_asset(&root, path) {
                AssetOutcome::Ok { content_type, body } => {
                    bevy::log::debug!(
                        url = %request.url,
                        mime = %content_type,
                        bytes = body.len(),
                        "ozmux-ext static asset served"
                    );
                    CefSchemeResponse {
                        status: 200,
                        mime_type: content_type,
                        headers: Vec::new(),
                        body: CefSchemeBody::Bytes(body),
                    }
                }
                AssetOutcome::NotFound => CefSchemeResponse::not_found(),
                AssetOutcome::Forbidden => status_text(403, "forbidden asset path"),
                AssetOutcome::TooLarge => status_text(413, "asset too large"),
            },
            AssetSource::Legacy(endpoints) => match fetch(&endpoints, path) {
                Ok(r) => {
                    let mime = bare_mime(&r.content_type);
                    bevy::log::debug!(
                        url = %request.url,
                        status = r.status,
                        mime = %mime,
                        bytes = r.body.len(),
                        "ozmux-ext legacy asset served"
                    );
                    CefSchemeResponse {
                        status: r.status,
                        mime_type: mime,
                        headers: Vec::new(),
                        body: CefSchemeBody::Bytes(r.body),
                    }
                }
                Err(FetchError::NotReady) => {
                    bevy::log::debug!(url = %request.url, "ozmux-ext legacy endpoint not ready");
                    status_text(503, "extension not ready")
                }
                Err(e) => {
                    bevy::log::warn!(url = %request.url, error = %e, "ozmux-ext legacy fetch failed");
                    status_text(502, "extension fetch failed")
                }
            },
        }
    }
}
```

(e) Update `custom_scheme` (`scheme.rs:136`) signature from `registry: EndpointRegistry` to `registry: AssetSourceRegistry` (the body is unchanged — it already passes `registry` to `OzmuxExtScheme::new`).

```rust
#[cfg(feature = "cef")]
pub fn custom_scheme(registry: AssetSourceRegistry) -> CefCustomScheme {
```

- [ ] **Step 5: Rewrite the `scheme.rs` dispatch test for the new registry (and de-gate it)**

In `scheme.rs`'s `#[cfg(test)] mod tests`, replace the test `dispatch_resolves_registered_name_and_404s_unknown_even_after_late_insert` (currently `#[cfg(feature = "cef")]`, `scheme.rs:212-230`) with this non-gated version:

```rust
    #[test]
    fn dispatch_resolves_static_and_legacy_and_404s_unknown_after_late_insert() {
        use crate::host::{AssetSource, AssetSourceRegistry, ExtensionEndpoints};
        use std::path::PathBuf;
        let registry = AssetSourceRegistry::default();
        // unknown name → 404
        assert_eq!(
            resolve_request(&registry, "ozmux-ext://ghost/index.html").err(),
            Some(404)
        );
        // register AFTER construction → resolvable (handler reads live registry)
        registry.insert("memo", AssetSource::Static(PathBuf::from("/abs/memo")));
        let (source, path) =
            resolve_request(&registry, "ozmux-ext://memo/app.js").expect("registered");
        assert!(matches!(source, AssetSource::Static(p) if p == PathBuf::from("/abs/memo")));
        assert_eq!(path, "app.js");
        // empty path defaults to index.html (parse_url behavior preserved)
        let (_src, path2) = resolve_request(&registry, "ozmux-ext://memo").expect("registered");
        assert_eq!(path2, "index.html");
        // legacy name dispatches to a Legacy source
        registry.insert("md", AssetSource::Legacy(ExtensionEndpoints::default()));
        let (legacy, _p) =
            resolve_request(&registry, "ozmux-ext://md/index.html").expect("registered");
        assert!(matches!(legacy, AssetSource::Legacy(_)));
    }
```

- [ ] **Step 6: Update `cef_plugin` signature in `extension_render.rs`**

In `src/extension_render.rs`:

(a) Change the import (`extension_render.rs:15`) from:

```rust
use ozmux_extension_host::host::EndpointRegistry;
```

to:

```rust
use ozmux_extension_host::host::AssetSourceRegistry;
```

(b) Change `cef_plugin` (`extension_render.rs:69`) signature + doc to take the new registry (body unchanged — it already passes the arg to `custom_scheme`):

```rust
/// Builds the `CefPlugin` with the `ozmux-ext://` scheme bound to the shared
/// `AssetSourceRegistry` the extension manager populates: `Static(<dir>)` for
/// new-model extensions (served directly by Rust) and `Legacy(...)` for legacy
/// command-extensions. The handler reads the live registry on each request, so
/// entries registered after `CefPlugin::build()` resolve; unregistered names
/// 404. The `window.ozmux` bridge is intentionally NOT registered as a global
/// extension here; it is injected per-webview via `PreloadScripts` in
/// `finish_extension_setup` (see the NOTE there).
pub fn cef_plugin(registry: AssetSourceRegistry) -> CefPlugin {
    CefPlugin {
        custom_schemes: vec![custom_scheme(registry)],
        command_line_config: cef_command_line_config(),
        ..Default::default()
    }
}
```

- [ ] **Step 7: Update `main.rs` construction**

In `src/main.rs`:

(a) Change the import (`main.rs:36`) from `use ozmux_extension_host::host::EndpointRegistry;` to:

```rust
use ozmux_extension_host::host::AssetSourceRegistry;
```

(b) Change the two construction/wiring sites (`main.rs:45,56,82`):

```rust
    let registry = AssetSourceRegistry::default();
```

```rust
            cef_plugin(registry.clone()),
```

```rust
            ExtensionManagerPlugin::new(registry),
```

- [ ] **Step 8: Update `extension_manager.rs` to the new registry + insert `Static`/`Legacy`**

In `src/extension_manager.rs`:

(a) Imports (`extension_manager.rs:11-13`) — replace `EndpointRegistry` with `AssetSource, AssetSourceRegistry`:

```rust
use ozmux_extension_host::host::{
    AssetSource, AssetSourceRegistry, ExtensionEndpoints, LifecycleEvent, RuntimeRoot,
};
```

(b) `ExtensionRegistry.endpoints` field type + its doc (`extension_manager.rs:42-47`):

```rust
    /// Shared name → asset-source map read by the `ozmux-ext://` scheme. The
    /// scheme handler reads its own clone (passed to `cef_plugin` in `main`);
    /// the resource holds the canonical handle so it stays alive for the app's
    /// lifetime. Legacy command-extensions are `Legacy(...)`; new-model
    /// extensions are `Static(<dir>)` served directly by Rust.
    endpoints: AssetSourceRegistry,
```

(c) `ExtensionManagerPlugin.endpoints` field (`extension_manager.rs:53`) and `new` (`extension_manager.rs:59-61`):

```rust
pub(crate) struct ExtensionManagerPlugin {
    endpoints: AssetSourceRegistry,
}

impl ExtensionManagerPlugin {
    /// Builds the extension sharing `endpoints` with the CEF scheme handler so the
    /// handler reads the very registry the manager populates on launch.
    pub(crate) fn new(endpoints: AssetSourceRegistry) -> Self {
        Self { endpoints }
    }
```

(d) In `spawn_single_host` (the loop at `extension_manager.rs:78-88`), insert `Static` sources for new-model extensions:

```rust
                for extension in &extensions {
                    // NOTE: coexistence slice — an extension dir sharing a name with
                    // a launched legacy command-extension would clobber its asset
                    // source (last-write-wins). Skip + warn; Step 5 removes legacy.
                    if self.endpoints.get(&extension.name).is_some() {
                        tracing::warn!(name = %extension.name, "extension name collides with a legacy command-extension; skipping");
                        continue;
                    }
                    self.endpoints.insert(
                        extension.name.clone(),
                        AssetSource::Static(extension.dir.clone()),
                    );
                }
```

(e) In `Plugin::build` (the legacy spawn loop at `extension_manager.rs:111`), wrap the endpoint as `Legacy`:

```rust
                    // NOTE: register the name with an EMPTY Legacy endpoint at spawn
                    // so an early CEF fetch resolves the name but finds no socket yet
                    // (FetchError::NotReady → 503), instead of ECONNREFUSED (502) on a
                    // socket the child has not bound. The real socket path is published
                    // by `publish_ready_endpoints` on readiness.
                    endpoints.insert(name.clone(), AssetSource::Legacy(ExtensionEndpoints::default()));
```

(f) `publish_ready_endpoints` (`extension_manager.rs:215-225`) — fetch the legacy handle via `legacy_endpoint`:

```rust
fn publish_ready_endpoints(registry: Res<ExtensionRegistry>) {
    for (name, ext) in registry.extensions.iter() {
        while let Ok(event) = ext.events().try_recv() {
            if let LifecycleEvent::Ready = event
                && let Some(ep) = registry.endpoints.legacy_endpoint(name)
            {
                ep.set(ext.asset_sock_path().to_path_buf());
            }
        }
    }
}
```

(g) Update the test `endpoint_stays_unpublished_until_extension_is_ready` (`extension_manager.rs:303-349`) — it builds an `EndpointRegistry` directly. Replace the registry construction + assertions to use `AssetSourceRegistry` + `AssetSource::Legacy` + `legacy_endpoint`:

```rust
        let endpoints = AssetSourceRegistry::default();
        endpoints.insert("memo", AssetSource::Legacy(ExtensionEndpoints::default()));
        let mut extensions: HashMap<String, CommandExtension> = HashMap::new();
        extensions.insert("memo".into(), ext);

        let registered = endpoints.legacy_endpoint("memo").expect("name resolves at spawn");
        assert!(
            registered.get().is_none(),
            "before readiness the endpoint must resolve the name but have no socket (NotReady -> 503, not 502)"
        );
```

and the readiness wait loop's check (later in the same test):

```rust
            if endpoints.legacy_endpoint("memo").and_then(|ep| ep.get()).is_some() {
                break;
            }
```

- [ ] **Step 9: Confirm `EndpointRegistry` is fully gone**

Run: `grep -rn "EndpointRegistry" src crates`
Expected: no output. (If any remain, update them to `AssetSourceRegistry`.)

- [ ] **Step 10: Build + test the whole tree**

Run: `cargo build`
Expected: success (binary compiles with the new registry).

Run: `cargo test -p ozmux_extension_host`
Expected: PASS (includes the new `asset_registry_*` and `dispatch_resolves_static_and_legacy_*` tests).

- [ ] **Step 11: Lint + format**

Run: `cargo clippy --workspace --all-targets && cargo fmt`
Expected: no warnings.

- [ ] **Step 12: Commit**

```bash
git add crates/extension_host/src/host.rs crates/extension_host/src/scheme.rs crates/extension_host/src/lib.rs src/extension_render.rs src/main.rs src/extension_manager.rs
git commit -m "feat: serve new-model extension assets directly from Rust (decision C)

Replace EndpointRegistry with a source-discriminating AssetSourceRegistry
(name -> Static(PathBuf) | Legacy(ExtensionEndpoints)). The ozmux-ext://
scheme handler serves Static sources via the in-process asset resolver and
keeps the legacy socket fetch for Legacy sources (coexistence; removed in
Step 5)."
```

---

## Task 3: Drop the now-unused `assetRoot` from the Node-facing descriptor

Under decision C the Node host never serves assets, so `assetRoot` in the host-manifest JSON is dead data. Remove it from both ends together (Rust serializer + Node zod schema) and rebuild the bundle. The Rust asset registry is fed from `DiscoveredExtension.dir` (Task 2), not from this field, so functionality is unaffected.

> **Sequencing note:** the Rust and Node sides MUST change together — if Rust stops emitting `assetRoot` while the bundled `assets/host.mjs` zod schema still requires it, the host fails to parse its manifest and never becomes ready. Do all sub-steps, then rebuild, in one commit.

**Files:**
- Modify: `crates/extension_host/src/host_descriptor.rs`
- Modify: `host/src/descriptors.ts`
- Modify: `host/src/load.ts` (only if it reads `assetRoot`)
- Regenerate: `assets/host.mjs`

- [ ] **Step 1: Inspect the Node descriptor contract**

Run: `grep -rn "assetRoot\|asset_root" host/src crates/extension_host/src`
Expected: shows `ExtensionDescriptorJson.asset_root` (Rust) and `assetRoot` in `host/src/descriptors.ts` (zod) — and possibly `host/src/load.ts`. Note every site for the edits below.

- [ ] **Step 2: Update the Rust descriptor test first (TDD — make it expect the field's absence)**

In `crates/extension_host/src/host_descriptor.rs`, change the `builds_camelcase_descriptor_with_absolute_paths` test (`host_descriptor.rs:131-140`) to expect JSON without `assetRoot`:

```rust
    #[test]
    fn builds_camelcase_descriptor_with_absolute_paths() {
        let built =
            BuiltHostManifest::new(&[extension("memo", "/abs/memo", &["api/fs.ts"], vec![])]);
        let json = serde_json::to_string(&built.manifest).unwrap();
        assert_eq!(
            json,
            r#"{"extensions":[{"name":"memo","apiPaths":["/abs/memo/api/fs.ts"]}]}"#
        );
    }
```

- [ ] **Step 3: Run the test to verify it FAILS**

Run: `cargo test -p ozmux_extension_host builds_camelcase_descriptor_with_absolute_paths`
Expected: FAIL — actual JSON still contains `"assetRoot":"/abs/memo"`.

- [ ] **Step 4: Remove `asset_root` from `ExtensionDescriptorJson`**

In `crates/extension_host/src/host_descriptor.rs`:

(a) Remove the field + its doc from the struct (`host_descriptor.rs:13-20`):

```rust
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionDescriptorJson {
    /// Extension name (the `ozmux-ext://<name>` host).
    pub name: String,
    /// Absolute paths of the extension's api `.ts` files (traversal-validated).
    pub api_paths: Vec<PathBuf>,
}
```

(b) Remove the `let asset_root = ...;` binding and the `asset_root,` field in the struct literal inside `BuiltHostManifest::new` (`host_descriptor.rs:47,56-60`):

```rust
        for extension in extensions {
            let mut api_paths = Vec::new();
            for rel in &extension.manifest.api {
                if is_safe_rel(rel) {
                    api_paths.push(extension.dir.join(rel));
                } else {
                    bevy::log::warn!(extension = %extension.name, path = %rel.display(), "unsafe api path; skipping");
                }
            }
            descriptors.push(ExtensionDescriptorJson {
                name: extension.name.clone(),
                api_paths,
            });
```

- [ ] **Step 5: Run the Rust test to verify it PASSES**

Run: `cargo test -p ozmux_extension_host builds_camelcase_descriptor_with_absolute_paths`
Expected: PASS.

- [ ] **Step 6: Remove `assetRoot` from the Node zod schema + any reads**

In `host/src/descriptors.ts`, remove the `assetRoot` property from the extension descriptor zod object (drop the `assetRoot: z.string()` line). If `host/src/load.ts` (or any other `host/src` file from Step 1) destructures/reads `assetRoot`, remove that read.

Run: `grep -rn "assetRoot" host/src`
Expected: no output.

- [ ] **Step 7: Run the host (Node) unit tests**

Run: `pnpm -C host test`
Expected: PASS (descriptor parse tests no longer reference `assetRoot`). If a host test asserts on `assetRoot`, update it to drop the field.

- [ ] **Step 8: Rebuild the embedded host bundle**

Run: `pnpm -C host build`
Expected: regenerates `assets/host.mjs` (large generated diff — expected).

- [ ] **Step 9: Build to confirm the embedded bundle still compiles in**

Run: `cargo build`
Expected: success (`assets/host.mjs` is `include_str!`d by `host_process.rs`).

- [ ] **Step 10: Lint + format**

Run: `cargo clippy -p ozmux_extension_host --all-targets && cargo fmt && pnpm -C host lint`
Expected: no warnings. (If `pnpm -C host lint` is not a script, use `pnpm lint` at the repo root.)

- [ ] **Step 11: Commit**

```bash
git add crates/extension_host/src/host_descriptor.rs host/src/descriptors.ts host/src/load.ts assets/host.mjs
git commit -m "refactor: drop now-unused assetRoot from the Node host descriptor

Decision C serves assets from Rust, so the Node host never reads assetRoot;
remove it from ExtensionDescriptorJson and the Node zod schema together and
rebuild assets/host.mjs. The Rust asset registry is fed from
DiscoveredExtension.dir, so behavior is unchanged."
```

---

## Done criteria

- `cargo test -p ozmux_extension_host` passes, including `asset::tests::*`, `asset_registry_distinguishes_static_and_legacy`, and `dispatch_resolves_static_and_legacy_*`.
- `cargo build` succeeds; `grep -rn EndpointRegistry src crates` is empty.
- `pnpm -C host test` passes; `grep -rn assetRoot host/src` is empty; `assets/host.mjs` rebuilt.
- New-model extensions (discovered via `ozmux.toml`) are registered as `AssetSource::Static(<dir>)` and the `ozmux-ext://` handler serves their files directly with a traversal guard and inferred MIME; legacy command-extensions keep the socket `fetch` path.
- E2E rendering of a real extension page is intentionally out of scope — it lands in Step 6 (the `@memo` migration provides the fixture).
