# Phase 1 — Step 3b-2: Single Host Spawn + ExtensionManagerPlugin Reshape Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Spawn exactly one `node main.ts` host (the Step 3a entry), wired Rust↔host: write the 3b-1 descriptor JSON, set `OZMUX_HOST_{RPC_SOCK,MANIFEST,READY_PATH}`, poll the ready file, populate `ViewRegistry` from the manifest views (capabilities live), and register each plugin name → the one host endpoint. **First invasive step** — the app now also runs a single host process at startup.

**Architecture / slicing decision (for spec-review):** 3b-2 spawns the host **alongside** the legacy per-extension model rather than ripping it out — this keeps the build green and `@memo` alive while the new host stands up. The single host binds only its RPC socket (Step 3a `main.ts`); **no asset serving yet** (3b-3), so an OSC-mounted plugin view is blank until 3b-3 + the Step 4 bridge land. Step 5 removes the legacy per-extension spawn + control plane. The hard-to-unit-test `node` spawn is kept to thin glue; the testable logic (descriptor write + env assembly + `ViewRegistry` population) is factored into pure functions.

**Known limitations (spec-review, accepted for this slice):**
- **Shared `EndpointRegistry` namespace:** plugins and legacy extensions key the same registry by bare name. A plugin whose name collides with a launched extension is **skipped + warned** (Task 3) so it can't clobber the extension's asset endpoint. The collision set is empty in practice (plugins live under `plugins/`, extensions under `extensions/`, and no `plugins/` example exists until Step 6). Step 5 dissolves the shared namespace.
- **`CARGO_MANIFEST_DIR` host-entry path** bakes a dev-tree path into the binary (valid under `cargo run`, not a shipped binary). This is the **same pre-existing pattern** the bundled `extensions/` resolver uses — consistent tech debt, not a new regression; packaging is out of scope for Phase 1.

**Tech Stack:** Rust 2024, Bevy 0.18, `serde_json`, the existing `RuntimeRoot`/`run_lifecycle`/`EndpointRegistry` from `extension_host`.

---

## Where this fits

Steps 1 ✅ · 2 ✅ · 3a ✅ · 3b-1 ✅. **3b-2 (this plan)** = spawn one host + populate `ViewRegistry`. **3b-3** = asset `{plugin,path}` protocol (host binds an asset socket; scheme routes by plugin). Step 4 = webview host-API bridge. Step 5 = remove legacy. Step 6 = memo plugin + E2E.

Reuses (verified in the 3b-1 map): `RuntimeRoot::resolve_in(parent, pid, name)` → `bin_dir()` (0700) + `socket_path(name)`; `run_lifecycle(timeout, is_ready, on_ready, child, shutdown, tx)` polling a closure (`move || ready_path.exists()`); `EndpointRegistry::insert(name, ExtensionEndpoints)` + `ExtensionEndpoints::set(path)`; `discover_plugins(roots)` + `build_host_manifest(&plugins)` (3b-1); `ViewRegistry::register(view_id, RegisteredView)` (Step 1).

Spec: `docs/.../2026-06-11-phase1-single-host-process-design.md`. Conventions: `.claude/rules/rust.md`. Library tests: `cargo test -p ozmux_extension_host`; binary smoke: `cargo build` (do NOT rely on `cargo test` for the node spawn — that is integration/Step-6 E2E).

---

## File Structure

| File | Responsibility | Action |
| --- | --- | --- |
| `crates/configs/src/path.rs` | add `plugins_dir(env)` + `PLUGINS_REL_PATH` | Modify |
| `crates/configs/src/lib.rs` | re-export `plugins_dir` if the crate re-exports path fns | Modify (if needed) |
| `crates/extension_host/src/host_process.rs` | `HostProcess` (spawn one node host) + pure `prepare_host_runtime` | Create |
| `crates/extension_host/src/lib.rs` | declare + re-export `host_process` | Modify |
| `src/extension_manager.rs` | reshape `build()`: discover plugins → spawn host → populate `ViewRegistry` + endpoints (keep legacy path) | Modify |

---

## Task 1: `plugins_dir` config root

**Files:** Modify `crates/configs/src/path.rs`.

- [ ] **Step 1: Failing test** — in `path.rs`'s test module, add (mirroring the existing `extensions_dir` tests):

```rust
    #[test]
    fn plugins_dir_prefers_xdg() {
        let env = FakeEnv {
            vars: HashMap::from([(ENV_XDG_CONFIG_HOME.to_string(), "/xdg".to_string())]),
            home: None,
        };
        assert_eq!(plugins_dir(&env).unwrap(), PathBuf::from("/xdg/ozmux/plugins"));
    }

    #[test]
    fn plugins_dir_falls_back_to_home() {
        let env = FakeEnv { vars: HashMap::new(), home: Some(PathBuf::from("/home/u")) };
        assert_eq!(plugins_dir(&env).unwrap(), PathBuf::from("/home/u/.config/ozmux/plugins"));
    }
```

> **REQUIRED (spec-review):** `FakeEnv` has NO builder methods — it is a struct literal `FakeEnv { vars: HashMap<...>, home: Option<PathBuf> }` (path.rs ~line 122). Copy the EXACT field names/types + construction from the existing `extensions_dir` tests in `path.rs` (~lines 198-216) and adapt the two tests above to match — the form above is best-effort and may need the real field names.

- [ ] **Step 2: Run, expect fail** — `cargo test -p ozmux_configs plugins_dir` → fail.

- [ ] **Step 3: Implement** — add the constant next to `EXTENSIONS_REL_PATH`:

```rust
const PLUGINS_REL_PATH: &str = "ozmux/plugins";
```

and the function next to `extensions_dir`:

```rust
/// Resolves the user plugins directory (`$XDG_CONFIG_HOME/ozmux/plugins` or
/// `~/.config/ozmux/plugins`).
pub fn plugins_dir(env: &dyn Env) -> OzmuxConfigsResult<PathBuf> {
    if let Some(xdg) = env.var(ENV_XDG_CONFIG_HOME) {
        return Ok(PathBuf::from(xdg).join(PLUGINS_REL_PATH));
    }
    if let Some(home) = env.home_dir() {
        return Ok(home.join(HOME_CONFIG_DIR).join(PLUGINS_REL_PATH));
    }
    Err(OzmuxConfigsError::HomeDirNotFound)
}
```

- [ ] **Step 4: Run, expect pass** + `cargo test -p ozmux_configs` green. **NOTE (spec-review):** `configs` does NOT re-export path fns at the crate root — `extensions_dir` is consumed as `ozmux_configs::path::extensions_dir`. Leave `plugins_dir` as a `pub fn` in the `path` module; the binary imports it as `ozmux_configs::path::plugins_dir`. No `lib.rs` change needed.

- [ ] **Step 5: Commit**

```bash
git add crates/configs/src/path.rs crates/configs/src/lib.rs
git commit -m "feat(configs): add plugins_dir config-root resolver"
```

---

## Task 2: `HostProcess` — spawn one node host

**Files:** Create `crates/extension_host/src/host_process.rs`; modify `lib.rs`.

> The pure `prepare_host_runtime` (writes the descriptor file, returns paths + env) is unit-tested; `HostProcess::spawn` is thin glue over `Command` + the existing `run_lifecycle`. `spawn` is NOT unit-tested (it launches `node`); it is exercised by the Step-6 E2E.

- [ ] **Step 1: Write the failing test** — create `host_process.rs` with the test module covering the pure helper:

```rust
//! Spawns the single Node host process (the `@ozmux/sdk/host` entry): writes the
//! descriptor JSON, sets the host env, and polls the ready file via run_lifecycle.

use crate::host::{LifecycleEvent, RuntimeRoot, run_lifecycle};
use bevy::log::error;
use crossbeam_channel::{Receiver, bounded};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn prepare_writes_descriptor_and_builds_env() {
        let runtime = tempdir().unwrap();
        let prepared = prepare_host_runtime(
            runtime.path(),
            r#"{"plugins":[]}"#,
        )
        .unwrap();

        // descriptor file written with the given JSON
        let written = std::fs::read_to_string(&prepared.manifest_path).unwrap();
        assert_eq!(written, r#"{"plugins":[]}"#);

        // env carries the three host vars pointing at the prepared paths
        let env: std::collections::HashMap<_, _> = prepared.env.iter().cloned().collect();
        assert_eq!(env["OZMUX_HOST_RPC_SOCK"], prepared.rpc_sock_path.to_string_lossy());
        assert_eq!(env["OZMUX_HOST_MANIFEST"], prepared.manifest_path.to_string_lossy());
        assert_eq!(env["OZMUX_HOST_READY_PATH"], prepared.ready_path.to_string_lossy());

        // ready file does NOT exist yet (the host writes it after binding)
        assert!(!prepared.ready_path.exists());
    }
}
```

- [ ] **Step 2: Run, expect fail** — `cargo test -p ozmux_extension_host host_process` → compile error.

- [ ] **Step 3: Implement** — insert above the test module:

```rust
/// The host's runtime paths + spawn env, with the descriptor JSON already written.
pub struct PreparedHost {
    /// RPC UDS the host binds (Rust connects here in Step 4).
    pub rpc_sock_path: PathBuf,
    /// Descriptor JSON file (`OZMUX_HOST_MANIFEST`) the host reads at startup.
    pub manifest_path: PathBuf,
    /// Ready marker file the host writes after binding; Rust polls its existence.
    pub ready_path: PathBuf,
    /// Env pairs to set on the child (`OZMUX_HOST_*`).
    pub env: Vec<(String, String)>,
}

/// Writes the descriptor JSON into `dir` and assembles the host's paths + env.
/// `dir` must be a 0700 runtime directory (e.g. `RuntimeRoot::bin_dir`).
pub fn prepare_host_runtime(dir: &Path, descriptor_json: &str) -> std::io::Result<PreparedHost> {
    let rpc_sock_path = dir.join("host.rpc.sock");
    let manifest_path = dir.join("host-manifest.json");
    let ready_path = dir.join(".host-ready");
    std::fs::write(&manifest_path, descriptor_json)?;
    let env = vec![
        ("OZMUX_HOST_RPC_SOCK".into(), rpc_sock_path.to_string_lossy().into_owned()),
        ("OZMUX_HOST_MANIFEST".into(), manifest_path.to_string_lossy().into_owned()),
        ("OZMUX_HOST_READY_PATH".into(), ready_path.to_string_lossy().into_owned()),
    ];
    Ok(PreparedHost { rpc_sock_path, manifest_path, ready_path, env })
}

/// A running single Node host process.
pub struct HostProcess {
    rpc_sock_path: PathBuf,
    events: Receiver<LifecycleEvent>,
    _runtime: RuntimeRoot,
    child: Arc<std::sync::Mutex<Option<std::process::Child>>>,
    shutdown: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl HostProcess {
    /// Spawns `node <entry>` with the host env, writing `descriptor_json` first
    /// and polling the ready file for up to `ready_timeout`.
    pub fn spawn(
        entry: OsString,
        runtime: RuntimeRoot,
        descriptor_json: &str,
        ready_timeout: Duration,
    ) -> std::io::Result<Self> {
        let prepared = prepare_host_runtime(runtime.bin_dir(), descriptor_json)?;
        let child = Command::new("node")
            .arg(&entry)
            .envs(prepared.env.iter().map(|(k, v)| (k.clone(), v.clone())))
            .stdin(Stdio::null())
            .spawn()?;
        let child = Arc::new(std::sync::Mutex::new(Some(child)));
        let shutdown = Arc::new(AtomicBool::new(false));
        let (tx, rx) = bounded::<LifecycleEvent>(8);
        let ready_path = prepared.ready_path.clone();
        let thread = std::thread::spawn({
            let child = Arc::clone(&child);
            let shutdown = Arc::clone(&shutdown);
            move || {
                run_lifecycle(ready_timeout, move || ready_path.exists(), || {}, child, shutdown, tx);
            }
        });
        Ok(Self {
            rpc_sock_path: prepared.rpc_sock_path,
            events: rx,
            _runtime: runtime,
            child,
            shutdown,
            thread: Some(thread),
        })
    }

    /// The RPC socket path the host binds.
    pub fn rpc_sock_path(&self) -> &Path {
        &self.rpc_sock_path
    }

    /// Lifecycle events (Ready / Exited) from the supervisor thread.
    pub fn events(&self) -> &Receiver<LifecycleEvent> {
        &self.events
    }
}

impl Drop for HostProcess {
    fn drop(&mut self) {
        self.shutdown.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
            // NOTE: reap the child after kill or it becomes a zombie; the
            // lifecycle thread guards on take(), so whichever takes the handle
            // first must wait(). Mirror CommandExtension::Drop (command.rs ~255-265).
            let _ = child.wait();
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
```

> **REQUIRED (spec-review):** open `crates/extension_host/src/command.rs` (~lines 255-265) and mirror `CommandExtension::Drop` exactly — including the kill+wait ordering and how it coordinates with the `run_lifecycle` thread's own handle access — so the host child is always reaped (no zombie) regardless of which side takes the `Child` first.

> **Verify against the real `host.rs`:** confirm `run_lifecycle` / `LifecycleEvent` / `RuntimeRoot` are `pub`/`pub(crate)` and importable as written; confirm the `run_lifecycle` signature matches (timeout, is_ready closure, on_ready closure, child Arc<Mutex<Option<Child>>>, shutdown AtomicBool, tx Sender). Adjust imports/paths to the actual visibility (the 3b-1 map shows these exist; match exact names). If `RuntimeRoot::bin_dir()` returns `&Path`, the `prepare_host_runtime(runtime.bin_dir(), ...)` call is correct. If any of these are `pub(crate)` and not exported, widen minimally or call within the crate.

- [ ] **Step 4: Declare + re-export** in `lib.rs`: `pub mod host_process;` + `pub use host_process::{HostProcess, PreparedHost, prepare_host_runtime};`.

- [ ] **Step 5: Run** — `cargo test -p ozmux_extension_host host_process` → PASS (1 test). `cargo test -p ozmux_extension_host` green. `cargo clippy -p ozmux_extension_host` + `cargo fmt` clean.

- [ ] **Step 6: Commit**

```bash
git add crates/extension_host/src/host_process.rs crates/extension_host/src/lib.rs
git commit -m "feat(extension_host): spawn the single node host process"
```

---

## Task 3: reshape `ExtensionManagerPlugin` to spawn the host + populate `ViewRegistry`

**Files:** Modify `src/extension_manager.rs`.

> Additive within the existing plugin: after the legacy per-extension spawn loop, add host discovery + spawn + `ViewRegistry` population + endpoint registration. The `register_views` population is factored out and unit-tested; the spawn is glue (verified by `cargo build` + Step-6 E2E).

- [ ] **Step 1: Write the failing test** — add a test (in `src/extension_manager.rs`'s test module, or create one) for the pure population helper:

```rust
    #[test]
    fn register_views_populates_registry_with_capabilities() {
        use ozmux_extension_host::{RegisteredView, ViewRegistry};
        let mut reg = ViewRegistry::default();
        register_views(
            &mut reg,
            vec![(
                "memo.main".to_string(),
                RegisteredView {
                    entry: "index.html".into(),
                    owning_ext: "memo".into(),
                    interactive: true,
                    capabilities: vec!["fs".into()],
                },
            )],
        );
        let v = reg.get("memo.main").expect("registered");
        assert_eq!(v.capabilities, vec!["fs".to_string()]);
        assert_eq!(v.owning_ext, "memo");
    }
```

- [ ] **Step 2: Run, expect fail** — `cargo test register_views` → fail (no `register_views`).

- [ ] **Step 3: Implement.**

(a) Add the helper (private fn in `extension_manager.rs`):

```rust
fn register_views(registry: &mut ViewRegistry, views: Vec<(String, RegisteredView)>) {
    for (view_id, view) in views {
        registry.register(view_id, view);
    }
}
```

(b) Add a `plugin_roots()` resolver (mirrors `discovery_roots`, user-first — user BEFORE bundled, the intentional reversal):

```rust
fn plugin_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    match plugins_dir(&SystemEnv) {
        Ok(dir) => roots.push(dir),
        Err(e) => tracing::warn!(error = %e, "could not resolve user plugins dir"),
    }
    roots.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("plugins"));
    roots
}
```

(c) In `ExtensionManagerPlugin::build`, AFTER the existing per-extension wiring, spawn the host and populate the registry. **Also DELETE the now-redundant `app.init_resource::<ViewRegistry>()`** (spec-review: `get_resource_or_init` below subsumes it). Insert:

```rust
        let plugins = discover_plugins(&plugin_roots());
        let built = build_host_manifest(&plugins);
        let descriptor_json =
            serde_json::to_string(&built.manifest).expect("host manifest serializes");
        // Populate the trust registry from manifests before the world starts.
        {
            let mut view_registry = app.world_mut().get_resource_or_init::<ViewRegistry>();
            register_views(&mut view_registry, built.views);
        }
        let host_entry: OsString = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("sdk/typescript/src/host/main.ts")
            .into_os_string();
        match RuntimeRoot::resolve_in(&std::env::temp_dir(), std::process::id(), "host")
            .map_err(|e| e.to_string())
            .and_then(|rt| {
                HostProcess::spawn(host_entry, rt, &descriptor_json, READY_TIMEOUT)
                    .map_err(|e| e.to_string())
            }) {
            Ok(host) => {
                for plugin in &plugins {
                    // NOTE: coexistence slice — a plugin sharing a name with a
                    // launched legacy extension would clobber its asset endpoint
                    // (last-write-wins). Skip + warn; Step 5 removes the legacy half.
                    if self.endpoints.get(&plugin.name).is_some() {
                        tracing::warn!(name = %plugin.name, "plugin name collides with a legacy extension; skipping");
                        continue;
                    }
                    self.endpoints.insert(plugin.name.clone(), ExtensionEndpoints::default());
                }
                app.insert_resource(HostRuntime { host });
            }
            Err(e) => tracing::error!(error = %e, "failed to spawn single host process"),
        }
        app.add_systems(Update, poll_host_lifecycle);
```

with the module-local timeout + handle resource:

```rust
const READY_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Resource)]
struct HostRuntime {
    host: HostProcess,
}
```

> **NOTE:** in 3b-2 the host serves no assets yet (3b-3), so plugin endpoints stay unset — CEF asset fetches get 503 "not ready" (no crash). 3b-3 sets each plugin endpoint to the host asset socket on Ready.

(d) Add a `poll_host_lifecycle` system so a host crash / `SpawnFailed` is not silent (spec-review: `events()` was otherwise never drained):

```rust
fn poll_host_lifecycle(host: Option<Res<HostRuntime>>) {
    let Some(host) = host else { return };
    while let Ok(event) = host.host.events().try_recv() {
        match event {
            LifecycleEvent::Ready => tracing::info!("single host process ready"),
            LifecycleEvent::SpawnFailed => tracing::error!("single host failed to become ready"),
            LifecycleEvent::Exited => tracing::warn!("single host process exited"),
        }
    }
}
```

> **Resolve imports (spec-review-corrected):**
> - `use ozmux_extension_host::{HostProcess, ViewRegistry, RegisteredView, discover_plugins, build_host_manifest};` — these ARE re-exported at the crate root.
> - `use ozmux_extension_host::host::{RuntimeRoot, ExtensionEndpoints, LifecycleEvent};` — these live in the `host` module, NOT re-exported at the root (the binary already imports `EndpointRegistry` this way in `main.rs`).
> - `use ozmux_configs::path::plugins_dir;` — path fns are NOT re-exported at the configs crate root.
> - `use std::ffi::OsString;` `use std::time::Duration;`.
> - Do NOT import `DEFAULT_READY_TIMEOUT` (private const in `command.rs`) — use the local `READY_TIMEOUT`.
> - Confirm the `LifecycleEvent` variant names against `host.rs` (`Ready`/`SpawnFailed`/`Exited`); adjust the match arms if they differ.

- [ ] **Step 4: Run** — `cargo test -p ozmux-gui register_views -- --test-threads=1` → PASS. `cargo build` → compiles (the spawn glue type-checks). `cargo clippy --workspace` + `cargo fmt` clean.

- [ ] **Step 5: Smoke (manual / optional in this task):** `cargo run` should start, spawn one `node` host (visible in `ps`), and not crash; legacy `@memo` still works. (Full behavior is E2E in Step 6; do not block the task on a webview being visible — assets are 3b-3.)

- [ ] **Step 6: Commit**

```bash
git add src/extension_manager.rs
git commit -m "feat: spawn the single host and populate ViewRegistry from plugin manifests"
```

---

## Done criteria for Step 3b-2

- `cargo test -p ozmux_extension_host` + `cargo test -p ozmux_configs` green; `cargo build` (workspace) compiles; clippy + fmt clean.
- `cargo run` starts, spawns exactly one `node` host process, does not crash; legacy terminals/`@memo` still function (coexistence).
- `ViewRegistry` is populated from discovered plugin manifests (capabilities live) — verified by the `register_views` unit test (and, once a `plugins/` example exists in Step 6, end-to-end).
- Asset serving + the webview bridge are NOT expected to work yet (3b-3 + Step 4).

After 3b-2 lands, **3b-3** extends `protocol::Request` with `plugin` (Rust `protocol.rs`/`scheme.rs` pass the parsed `<name>`; Node `serveAssets`/`fileAssetHandler` route by plugin to `assetRoot`), and the host binds an asset socket whose path each plugin endpoint is set to on Ready.

### 3b-3 carry-forward (from 3b-2)
- The host (`main.ts`) must additionally bind an **asset socket** at `OZMUX_HOST_ASSET_SOCK` and serve `{plugin, path}` → `<assetRoot>/<path>` (reuse `fileAssetHandler` per plugin keyed by the descriptor's `assetRoot`). `prepare_host_runtime` adds the asset sock path + env; `HostProcess` exposes `asset_sock_path()`.
- `poll_host_lifecycle` (added in 3b-2) is where the plugin endpoints get **set** on `LifecycleEvent::Ready` — point each plugin's `ExtensionEndpoints` at the host asset socket (currently they stay unset → 503).
- `protocol::Request { path }` → `{ plugin, path }`; bump the framing (a second length-prefixed field) on BOTH the Rust `write_request`/`read_request` (`protocol.rs`) and the Node `serveAssets` parser (`asset-server.ts`, currently `[version][u32 path_len][path]`).

## Status: Step 3b-2 COMPLETE (2026-06-11)

All 3 tasks landed, each through an independent spec+quality review; the plan was spec-reviewed (Codex + Claude) and corrected before implementation (caught: private `DEFAULT_READY_TIMEOUT`, wrong `FakeEnv`/import paths, endpoint collision, Drop reap, unread events). Commits `2ab55a1`, `359bcf6`, `c1f2732`. Evidence: `ozmux_extension_host` **72/72**, `ozmux_configs` **129/129**, `cargo build` (workspace) clean, clippy + fmt clean. **First invasive step** — the app now spawns one Node host at startup, alongside the legacy per-extension model (coexistence). The host finds zero plugins until the Step-6 example exists; serves no assets until 3b-3 — both safe (no panic, CEF 503).
