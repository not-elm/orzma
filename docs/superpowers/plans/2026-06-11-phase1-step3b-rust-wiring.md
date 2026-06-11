# Phase 1 — Step 3b: Rust Host Wiring Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire the Rust side to the runnable host from Step 3a: extend `ExtensionManifest` with the extension-level `api: Vec<String>`, discover `extensions/*` (user-first), build the `HostManifest` descriptor JSON + `ViewRegistry` entries (capabilities populated, `entry`/`id`/`api` validated), spawn exactly one `node main.ts`, and reshape the asset protocol to `{extension, path}` so one host serves all extensions.

**Architecture:** Rust owns discovery + `ozmux.toml` parsing + trust data. Slice 3b-1 (this plan, **additive** — nothing removed, app unaffected) adds the pure, unit-testable core: the manifest `api` field, an `extension_discovery` scan, and a `host_descriptor` builder that emits the exact camelCase JSON Step 3a's zod schema expects plus validated `RegisteredView` entries. Slices 3b-2 (spawn one host, reshape `ExtensionManagerPlugin`) and 3b-3 (asset `{extension,path}`) are the invasive parts, outlined below and authored just-in-time.

**Tech Stack:** Rust 2024, Bevy 0.18, `serde`/`serde_json` (already deps), `toml` (added Step 1), `tempfile` (dev-dep).

---

## Slices

- **3b-1 (this plan) — manifest field + discovery + descriptor/view builder** (Rust, additive, unit-tested). No process-model change.
- **3b-2 — single host spawn + `ExtensionManagerPlugin` reshape** (invasive): a `HostProcess` (analog of `CommandExtension`) spawning one `node host.mjs` with `OZMUX_HOST_{RPC_SOCK,MANIFEST,READY_PATH}`, writing the descriptor JSON into the runtime root, polling the ready file (reuse `run_lifecycle` with `move || ready_path.exists()`), and registering each extension name → the one host endpoint in `EndpointRegistry`. Populate `ViewRegistry` from the 3b-1 builder at startup. This supersedes the per-extension spawn; legacy `@memo` goes dark until Step 6.
- **3b-3 — asset `{extension, path}` protocol**: extend `protocol::Request` with `extension: String`; `scheme.rs` passes the parsed `<name>` through (`fetch` gains the extension); the Node `serveAssets`/`fileAssetHandler` read the extension and route to `assetRoot`. One asset socket, all extensions.

> **3b-1 → 3b-2/3b-3 contract:** `host_descriptor::HostManifestJson` serializes to exactly `{ "extensions": [{ "name", "apiPaths", "assetRoot" }] }` (camelCase) — the shape Step 3a's `parseHostManifest` zod schema validates. `apiPaths`/`assetRoot` are absolute. 3b-2 writes this struct to the `OZMUX_HOST_MANIFEST` file.

Spec: `docs/superpowers/specs/2026-06-11-phase1-single-host-process-design.md`. Plan deps: Step 3a (`docs/.../2026-06-11-phase1-step3a-host-server-and-loader.md`). Conventions: `.claude/rules/rust.md` (no mod.rs; comments TODO/NOTE/SAFETY; `///` on pub; imports one top block; private items last; mutable params first).

> Run library tests with `cargo test -p ozmux_extension_host <filter>` (unaffected by the binary-crate IME/SIGSEGV flake). `cargo clippy -p ozmux_extension_host` + `cargo fmt` before each commit.

---

## File Structure (3b-1)

| File | Responsibility | Action |
| --- | --- | --- |
| `crates/extension_host/src/extension_manifest.rs` | add `api: Vec<String>` to `ExtensionManifest`/`RawManifest` | Modify |
| `crates/extension_host/src/extension_discovery.rs` | `discover_extensions(roots)` → `Vec<DiscoveredExtension>` (user-first, dedup) | Create |
| `crates/extension_host/src/host_descriptor.rs` | `build_host_manifest` → camelCase JSON struct + validated `RegisteredView` entries | Create |
| `crates/extension_host/src/lib.rs` | declare + re-export the two new modules | Modify |

---

## Task 1: add the extension-level `api` field to `ExtensionManifest`

**Files:** Modify `crates/extension_host/src/extension_manifest.rs`.

- [ ] **Step 1: Add a failing test** — in `extension_manifest.rs`'s `#[cfg(test)] mod tests`, add:

```rust
    #[test]
    fn parses_extension_level_api_files() {
        let text = r#"
api = ["api/fs.ts", "api/net.ts"]

[[views]]
id = "memo.main"
entry = "index.html"
"#;
        let m = ExtensionManifest::parse(text).unwrap();
        assert_eq!(m.api, vec!["api/fs.ts".to_string(), "api/net.ts".to_string()]);
        assert_eq!(m.views.len(), 1);
    }

    #[test]
    fn api_defaults_to_empty() {
        let m = ExtensionManifest::parse("[[views]]\nid = \"v\"\nentry = \"a.html\"\n").unwrap();
        assert!(m.api.is_empty());
    }
```

- [ ] **Step 2: Run, expect fail** — `cargo test -p ozmux_extension_host extension_manifest` → compile error (`ExtensionManifest` has no field `api`).

- [ ] **Step 3: Implement** — add the field to `ExtensionManifest` (after `views`):

```rust
/// An extension's resolved manifest: the views it publishes for OSC mounting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionManifest {
    /// Extension-relative paths of the api `.ts` files this extension loads (multiple allowed).
    pub api: Vec<String>,
    /// Views this extension publishes, addressable by `view_id` from OSC mounts.
    pub views: Vec<ExtensionView>,
}
```

Update `parse` to carry `api`:

```rust
    pub fn parse(text: &str) -> Result<Self, ExtensionManifestError> {
        let raw: RawManifest = toml::from_str(text).map_err(ExtensionManifestError::Toml)?;
        let views = raw
            .views
            .into_iter()
            .map(|v| ExtensionView {
                id: v.id,
                entry: v.entry,
                capabilities: v.capabilities,
                interactive: v.interactive,
            })
            .collect();
        Ok(Self { api: raw.api, views })
    }
```

Add the field to `RawManifest`:

```rust
#[derive(Deserialize)]
struct RawManifest {
    #[serde(default)]
    api: Vec<String>,
    #[serde(default)]
    views: Vec<RawView>,
}
```

- [ ] **Step 4: Run, expect pass** — `cargo test -p ozmux_extension_host extension_manifest` → PASS (existing + 2 new). The other `ExtensionManifest { .. }` literals (in tests only) need `api: vec![]` if any fail to compile — there are none outside this file's tests, but the existing tests construct via `parse`, so no literal updates needed.

- [ ] **Step 5: Commit**

```bash
git add crates/extension_host/src/extension_manifest.rs
git commit -m "feat(extension_host): add extension-level api file list to ExtensionManifest"
```

---

## Task 2: extension discovery (`extension_discovery.rs`)

**Files:** Create `crates/extension_host/src/extension_discovery.rs`; modify `lib.rs`.

> Mirrors `src/extension_manager.rs::discover_extensions` but scans for `ozmux.toml` (not `package.json`) and returns the parsed manifest. Pure over an input `roots: &[PathBuf]`, so it is unit-testable with `tempfile` fixtures. Caller passes roots **user-first**; dedup keeps the first occurrence (so user wins).

- [ ] **Step 1: Write the failing test** — create `extension_discovery.rs` with the test module first:

```rust
//! Scans extension directories for `ozmux.toml`, returning each extension's parsed
//! manifest. Pure over the given roots; the caller orders roots user-first.

use crate::extension_manifest::ExtensionManifest;
use std::path::{Path, PathBuf};

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn write_plugin(root: &Path, name: &str, toml: &str) {
        let dir = root.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("ozmux.toml"), toml).unwrap();
    }

    #[test]
    fn discovers_plugins_with_manifests_sorted() {
        let root = tempdir().unwrap();
        write_plugin(root.path(), "b", "api = [\"a.ts\"]\n");
        write_plugin(root.path(), "a", "api = [\"a.ts\"]\n");
        fs::create_dir_all(root.path().join("no-manifest")).unwrap();
        let found = discover_extensions(&[root.path().to_path_buf()]);
        assert_eq!(found.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(found[0].manifest.api, vec!["a.ts".to_string()]);
    }

    #[test]
    fn first_root_wins_on_duplicate_name() {
        let user = tempdir().unwrap();
        let bundled = tempdir().unwrap();
        write_plugin(user.path(), "memo", "api = [\"user.ts\"]\n");
        write_plugin(bundled.path(), "memo", "api = [\"bundled.ts\"]\n");
        // user root passed first → user wins
        let found = discover_extensions(&[user.path().to_path_buf(), bundled.path().to_path_buf()]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].manifest.api, vec!["user.ts".to_string()]);
    }

    #[test]
    fn skips_malformed_manifest() {
        let root = tempdir().unwrap();
        write_plugin(root.path(), "good", "api = [\"a.ts\"]\n");
        write_plugin(root.path(), "bad", "this = = not toml");
        let found = discover_extensions(&[root.path().to_path_buf()]);
        assert_eq!(found.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["good"]);
    }

    #[test]
    fn missing_root_is_ignored() {
        let found = discover_extensions(&[PathBuf::from("/nonexistent-ozmux-root")]);
        assert!(found.is_empty());
    }
}
```

- [ ] **Step 2: Run, expect fail** — `cargo test -p ozmux_extension_host extension_discovery` → compile error.

- [ ] **Step 3: Implement** — insert above the test module:

```rust
/// A discovered extension: its name (directory name), absolute directory, and parsed manifest.
#[derive(Debug, Clone)]
pub struct DiscoveredExtension {
    /// Extension name = its directory name (the `ozmux-ext://<name>` host).
    pub name: String,
    /// Absolute extension directory (asset root + base for api paths).
    pub dir: PathBuf,
    /// The parsed `ozmux.toml`.
    pub manifest: ExtensionManifest,
}

/// Scans each root for immediate subdirectories containing an `ozmux.toml`,
/// returning the parsed extensions. Within a root, results are sorted by name;
/// across roots, the first occurrence of a name wins (caller passes user roots
/// first). Unreadable roots and malformed/invalid manifests are skipped with a log.
pub fn discover_extensions(roots: &[PathBuf]) -> Vec<DiscoveredExtension> {
    let mut found = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for root in roots {
        let entries = match std::fs::read_dir(root) {
            Ok(e) => e,
            Err(_) => continue,
        };
        let mut dirs: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
        dirs.sort();
        for dir in dirs {
            let manifest_path = dir.join("ozmux.toml");
            if !manifest_path.is_file() {
                continue;
            }
            let Some(name) = dir.file_name().and_then(|n| n.to_str()).map(str::to_string) else {
                continue;
            };
            let text = match std::fs::read_to_string(&manifest_path) {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!(path = %manifest_path.display(), error = %e, "failed to read ozmux.toml");
                    continue;
                }
            };
            let manifest = match ExtensionManifest::parse(&text) {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!(path = %manifest_path.display(), error = %e, "failed to parse ozmux.toml");
                    continue;
                }
            };
            if !seen.insert(name.clone()) {
                tracing::warn!(name = %name, "duplicate extension name; keeping first occurrence");
                continue;
            }
            found.push(DiscoveredExtension { name, dir, manifest });
        }
    }
    found
}
```

- [ ] **Step 4: Declare + re-export in `lib.rs`** — add `pub mod extension_discovery;` (alphabetical, after `pub mod extension_manifest;`... actually `extension_discovery` sorts before `extension_manifest`) and the re-export `pub use extension_discovery::{DiscoveredExtension, discover_extensions};` after the `extension_manifest` re-export.

- [ ] **Step 5: Run, expect pass** — `cargo test -p ozmux_extension_host extension_discovery` → PASS (4 tests).

- [ ] **Step 6: Commit**

```bash
git add crates/extension_host/src/extension_discovery.rs crates/extension_host/src/lib.rs
git commit -m "feat(extension_host): discover extensions from ozmux.toml (user-first)"
```

---

## Task 3: host-manifest + view builder (`host_descriptor.rs`)

**Files:** Create `crates/extension_host/src/host_descriptor.rs`; modify `lib.rs`.

> Turns discovered extensions into (a) the camelCase `HostManifestJson` Rust serializes to the `OZMUX_HOST_MANIFEST` file (matching Step 3a's zod schema), and (b) validated `(view_id, RegisteredView)` entries for `ViewRegistry`. Validation (carried from the Step 1/2 reviews): reject `entry`/api paths containing a `..` component or empty, and `view_id`s that are empty or contain whitespace. Invalid items are skipped with a warning (fail-soft), not fatal.

- [ ] **Step 1: Write the failing test** — create `host_descriptor.rs` with the test module first:

```rust
//! Builds the host-manifest descriptor JSON (consumed by the Node host) and the
//! capability-bearing `ViewRegistry` entries from discovered extensions.

use crate::extension_discovery::DiscoveredExtension;
use crate::extension_manifest::{ExtensionManifest, ExtensionView};
use crate::registry::RegisteredView;
use serde::Serialize;
use std::path::PathBuf;

#[cfg(test)]
mod tests {
    use super::*;

    fn extension(name: &str, dir: &str, api: &[&str], views: Vec<ExtensionView>) -> DiscoveredExtension {
        DiscoveredExtension {
            name: name.into(),
            dir: PathBuf::from(dir),
            manifest: ExtensionManifest { api: api.iter().map(|s| s.to_string()).collect(), views },
        }
    }

    fn view(id: &str, entry: &str, caps: &[&str]) -> ExtensionView {
        ExtensionView {
            id: id.into(),
            entry: entry.into(),
            capabilities: caps.iter().map(|s| s.to_string()).collect(),
            interactive: true,
        }
    }

    #[test]
    fn builds_camelcase_descriptor_with_absolute_paths() {
        let built = build_host_manifest(&[extension("memo", "/abs/memo", &["api/fs.ts"], vec![])]);
        let json = serde_json::to_string(&built.manifest).unwrap();
        assert_eq!(
            json,
            r#"{"extensions":[{"name":"memo","apiPaths":["/abs/memo/api/fs.ts"],"assetRoot":"/abs/memo"}]}"#
        );
    }

    #[test]
    fn builds_view_entries_with_capabilities() {
        let built =
            build_host_manifest(&[extension("memo", "/abs/memo", &[], vec![view("memo.main", "index.html", &["fs"])])]);
        assert_eq!(built.views.len(), 1);
        let (id, rv) = &built.views[0];
        assert_eq!(id, "memo.main");
        assert_eq!(rv.owning_ext, "memo");
        assert_eq!(rv.entry, "index.html");
        assert_eq!(rv.capabilities, vec!["fs".to_string()]);
        assert!(rv.interactive);
    }

    #[test]
    fn rejects_path_traversal_in_entry_and_api() {
        let built = build_host_manifest(&[extension(
            "bad",
            "/abs/bad",
            &["../escape.ts"],
            vec![view("bad.v", "../../etc/passwd", &[])],
        )]);
        // the traversing api path is dropped; no view is registered
        assert!(built.manifest.extensions[0].api_paths.is_empty());
        assert!(built.views.is_empty());
    }

    #[test]
    fn rejects_empty_or_whitespace_view_id() {
        let built = build_host_manifest(&[extension(
            "p",
            "/abs/p",
            &[],
            vec![view("", "a.html", &[]), view("has space", "b.html", &[])],
        )]);
        assert!(built.views.is_empty());
    }
}
```

- [ ] **Step 2: Run, expect fail** — `cargo test -p ozmux_extension_host host_descriptor` → compile error.

- [ ] **Step 3: Implement** — insert above the test module:

```rust
/// One extension's load + serve descriptor, serialized as camelCase to match the
/// Node host's `parseHostManifest` zod schema.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionDescriptorJson {
    /// Extension name (the `ozmux-ext://<name>` host).
    pub name: String,
    /// Absolute paths of the extension's api `.ts` files (traversal-validated).
    pub api_paths: Vec<String>,
    /// Absolute extension directory the host serves assets from.
    pub asset_root: String,
}

/// The host-manifest JSON Rust writes to `OZMUX_HOST_MANIFEST`.
#[derive(Debug, Clone, Serialize)]
pub struct HostManifestJson {
    /// One descriptor per discovered extension.
    pub extensions: Vec<ExtensionDescriptorJson>,
}

/// The descriptor JSON plus the `ViewRegistry` entries to register.
#[derive(Debug, Clone)]
pub struct BuiltHostManifest {
    /// Serialized to the `OZMUX_HOST_MANIFEST` file for the Node host.
    pub manifest: HostManifestJson,
    /// `(view_id, RegisteredView)` pairs to insert into `ViewRegistry`.
    pub views: Vec<(String, RegisteredView)>,
}

/// Builds the descriptor JSON + validated view entries from discovered extensions.
/// A relative path component (`..`) in an api or entry path, and an empty or
/// whitespace-bearing `view_id`, are rejected (skipped with a warning) — the
/// trust boundary that keeps PTY/manifest data from escaping the extension dir.
pub fn build_host_manifest(extensions: &[DiscoveredExtension]) -> BuiltHostManifest {
    let mut descriptors = Vec::new();
    let mut views = Vec::new();
    for extension in extensions {
        let asset_root = extension.dir.to_string_lossy().into_owned();
        let mut api_paths = Vec::new();
        for rel in &extension.manifest.api {
            if is_safe_rel(rel) {
                api_paths.push(extension.dir.join(rel).to_string_lossy().into_owned());
            } else {
                tracing::warn!(extension = %extension.name, path = %rel, "unsafe api path; skipping");
            }
        }
        descriptors.push(ExtensionDescriptorJson { name: extension.name.clone(), api_paths, asset_root });
        for view in &extension.manifest.views {
            if view.id.is_empty() || view.id.chars().any(char::is_whitespace) {
                tracing::warn!(extension = %extension.name, id = %view.id, "invalid view id; skipping");
                continue;
            }
            if !is_safe_rel(&view.entry) {
                tracing::warn!(extension = %extension.name, entry = %view.entry, "unsafe view entry; skipping");
                continue;
            }
            views.push((
                view.id.clone(),
                RegisteredView {
                    entry: view.entry.clone(),
                    owning_ext: extension.name.clone(),
                    interactive: view.interactive,
                    capabilities: view.capabilities.clone(),
                },
            ));
        }
    }
    BuiltHostManifest { manifest: HostManifestJson { extensions: descriptors }, views }
}

/// True when `rel` is a non-empty relative path with no `..` component and no leading `/`.
fn is_safe_rel(rel: &str) -> bool {
    !rel.is_empty()
        && !rel.starts_with('/')
        && std::path::Path::new(rel)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
}
```

- [ ] **Step 4: Declare + re-export in `lib.rs`** — `pub mod host_descriptor;` (after `pub mod host;`) + `pub use host_descriptor::{BuiltHostManifest, HostManifestJson, ExtensionDescriptorJson, build_host_manifest};`.

- [ ] **Step 5: Run, expect pass** — `cargo test -p ozmux_extension_host host_descriptor` → PASS (4 tests). Then `cargo test -p ozmux_extension_host` → all green; `cargo clippy -p ozmux_extension_host` + `cargo fmt` clean.

- [ ] **Step 6: Commit**

```bash
git add crates/extension_host/src/host_descriptor.rs crates/extension_host/src/lib.rs
git commit -m "feat(extension_host): build host-manifest JSON + validated view entries"
```

---

## Done criteria for Step 3b-1

- `cargo test -p ozmux_extension_host` green (manifest `api` field, discovery, descriptor builder).
- `cargo clippy -p ozmux_extension_host` + `cargo fmt --check` clean.
- Purely additive: no spawn/process change, no existing behavior removed, app still builds and runs the legacy model. `discover_extensions`/`build_host_manifest` have no production caller yet (3b-2 calls them).
- The descriptor serializes to the exact camelCase shape Step 3a's zod schema accepts (verified by the round-trip test).

After 3b-1 lands, **3b-2** is authored against `command.rs`/`host.rs`/`extension_manager.rs`/`main.rs`: a `HostProcess` spawning one `node host.mjs` with `OZMUX_HOST_{RPC_SOCK,MANIFEST,READY_PATH}` (write `serde_json::to_string(&built.manifest)` to the manifest file; reuse `run_lifecycle` with `move || ready_path.exists()`), reshape `ExtensionManagerPlugin` to spawn it + `init` `ViewRegistry` from `built.views` + register every extension name → the one host endpoint. Then **3b-3** extends `protocol::Request` with `extension` and routes assets by extension.

### 3b-2 carry-forward (from 3b-1 reviews)
- **Discovery roots:** the always-on root is `extensions_dir(env)` (`~/.config/ozmux/extensions`); the project-root bundled dir `<CARGO_MANIFEST_DIR>/extensions` is added only under `#[cfg(feature = "debug")]`. Pass user-first so `discover_extensions`'s first-wins yields user override; discovery reuses `extensions_dir` (no separate const).
- **Shadow log level:** when wiring discovery, the duplicate-name `warn!` in `discover_extensions` fires on every *intended* user override — consider lowering to `debug!` (it's designed behavior, not an error) before it runs in production.
- **`node` entry path:** the host runtime is bundled by esbuild to `assets/host.mjs`, embedded via `include_str!`, written to the runtime dir as `host.mjs`, and spawned as `node host.mjs` (the host loads extension api files by the ABSOLUTE `apiPaths` in the descriptor, so its cwd is not load-bearing).
- **3b-2 is invasive** — it supersedes the per-extension spawn; run a spec-review on the 3b-2 plan before implementing (per the 3a precedent).

## Status: Step 3b-1 COMPLETE (2026-06-11)

All 3 tasks landed, each through an independent spec+quality review. Commits `3f85353`, `8855bf0`, `2202a56`. Evidence: `ozmux_extension_host` lib **71/71**, `cargo clippy` clean, `cargo fmt --check` clean. Purely additive — no spawn/process change; `discover_extensions`/`build_host_manifest` have no production caller yet (3b-2 wires them). The descriptor JSON shape was verified (in the Task 3 review) to exactly match Step 3a's zod `parseHostManifest` schema — the cross-language handoff contract is intact.
