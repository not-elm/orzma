# Phase 1 Step 6: New-Model `memo` Migration + E2E Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Convert the bundled `extensions/memo` from the legacy `bootstrap()`/`@memo`-command model to the new host-API model (`api.ts` + `ozmux.toml`), and prove the full new path with an E2E test: a real Node host loads memo's `api.ts`, a capability-gated `fs.read` round-trips back to the calling webview as binary, and an ungranted namespace is rejected.

**Architecture:** Spec `docs/superpowers/specs/2026-06-11-phase1-single-host-process-design.md` §4④ (実装ステップ順序・移行スコープ; Step 6 runs **before** Step 5 legacy removal). The Rust→host→ViewRegistry→GrantedNamespaces→host-bridge wiring is already in place (Steps 1–4, PR #97). This step adds only the memo **data files** (`api.ts`/`ozmux.toml`/`index.html`) plus an integration test; it does **not** touch `host/src` or Rust production wiring (so `assets/host.mjs` stays unchanged). memo's legacy files are deleted here so the legacy `package.json`-based discovery no longer double-registers memo against the new `ozmux.toml`-based discovery.

**Tech Stack:** TypeScript (erasable, ESM — Node ≥23.6 native type-stripping), TOML manifest, Rust (Bevy 0.18 headless test app, `ozmux_extension_host` host-spawn + NDJSON RPC client), `node` (spawned host runtime).

**Key facts (verified against current code):**
- New-model discovery scans `ozmux.toml` (`crates/extension_host/src/extension_discovery.rs::discover_extensions`); `BuiltHostManifest::new` (`host_descriptor.rs`) turns each manifest into (a) the `OZMUX_HOST_MANIFEST` JSON with absolute `apiPaths` and (b) `(ViewId, RegisteredView)` entries (with `capabilities`). `src/extension_manager.rs:71-72` calls `register_views` into `ViewRegistry` at startup. **All of this already exists** — memo is just missing its `ozmux.toml`.
- The host runtime (`host/src/main.ts` → `assets/host.mjs`, embedded via `include_str!` in `host_process.rs:15`) reads the manifest, `import()`s every extension's `apiPaths`, merges namespaces (`extension-loader.ts::mergeApis`, first-wins), and dispatches `api[ns][method](...args)` (`dispatch.ts`), encoding a top-level `Uint8Array`/`Buffer` result as `{__u8:"<base64>"}` (`binary-codec.ts::encodeHostValue`).
- The capability gate (`src/extension_render.rs::on_host_call_frame`) trusts the `frame.webview` entity's `GrantedNamespaces` (never the JS payload). Allowed calls forward over `HostRpcClient` with a Rust-minted global `reqId`; `drain_host_rpc_responses` maps the reply back to `(webview, pageReqId)` and re-emits on the `"ozmux"` channel. Replies are observable in tests via the existing `CapturedEmits`/`capture_emits`/`gate_app` harness (`extension_render.rs:1182-1199`).
- **Legacy discovery requires `name`:** `Manifest::parse` (`manifest.rs:14-19`) returns `MissingName` when `package.json` has no `name`. So a `package.json` of `{"type":"module"}` (no name) is **skipped** by `discover_command_extensions` (`extension_manager.rs:165`) — memo stops being a legacy extension — while still telling Node that `api.ts` is **ESM** (the repo-root `package.json` has **no `type` field**, i.e. CommonJS by default, so the explicit `type:"module"` is load-bearing for `export default` to parse).
- The legacy spawn test `endpoint_stays_unpublished_until_extension_is_ready` + helpers `node_and_memo_available` / `memo_dir` (`extension_manager.rs:315-381`) spawn memo as a **legacy** command-extension (`node bootstrap.ts`). Deleting memo's `bootstrap.ts`/`package.json` breaks it (it would either skip — if `node_modules/@ozmux/sdk` is absent — or panic on spawn). It is removed in Task 3.
- `HostProcess::spawn(RuntimeRoot, &descriptor_json, timeout)` writes the embedded `host.mjs`, runs `node host.mjs`, and emits `LifecycleEvent::Ready` once the `.host-ready` file appears (`host_process.rs:78-130`). `host.events()` is a crossbeam `Receiver`; `host.rpc_sock_path()` feeds `HostRpcClient::connect`.

**Verification commands:**
- Binary (gui) Rust tests: `cargo test -p ozmux-gui --bin ozmux-gui -- --test-threads=1`
- Host-crate Rust tests: `cargo test -p ozmux_extension_host`
- Host (Node) tests: `pnpm -C host test`
- TS typecheck (memo): `pnpm -C extensions/memo check-types` (after Task 1 wires its tsconfig include) — OR `pnpm check-types` workspace-wide
- Full build + lint: `cargo build && cargo clippy --workspace --all-targets && cargo fmt --check`
- NOTE: a full `cargo test` has a pre-existing IME failure + a parallel-teardown SIGSEGV unrelated to this work; use the per-crate commands above and `--test-threads=1` for the gui crate.

---

## File Structure

**Create:**
- `extensions/memo/api.ts` — memo's host API: one `fs` namespace whose `read(path)` returns the file's bytes. Erasable TS, ESM, no `@ozmux/sdk` import (plain `export default`).
- `extensions/memo/ozmux.toml` — the new-model manifest: `api = ["api.ts"]` + a single `memo.main` view (`entry = "index.html"`, `capabilities = ["fs"]`, `interactive = true`).

**Modify:**
- `extensions/memo/package.json` — replace the legacy package (name `memo`, `@ozmux/sdk` dep) with the minimal `{"type":"module"}` (no `name`, so legacy discovery skips it; `type:"module"` so `api.ts` loads as ESM).
- `extensions/memo/index.html` — replace the `window.ozmux` handler/channel demo with a `window.fs.read` demo.
- `extensions/memo/tsconfig.json` — re-point `include` from `bootstrap.ts` to `api.ts` (kept for `check-types`), OR delete it (see Task 3 decision).
- `src/extension_render.rs` — add the E2E test (`#[cfg(test)]`, node-gated) to the existing `mod tests`.
- `src/extension_manager.rs` — add two manifest/discovery tests; remove the legacy memo spawn test + its `node_and_memo_available`/`memo_dir` helpers.

**Delete:**
- `extensions/memo/bootstrap.ts` — the legacy `bootstrap({commands:{'@memo':…}})` entry, replaced by `api.ts` + OSC mount.

**Untouched (call out in self-review):**
- `host/src/**` and `assets/host.mjs` — the host loader already imports `api.ts` generically; no host change, so **do not** rebuild `host.mjs`.
- `src/osc_webview.rs` — the OSC-mount → `GrantedNamespaces` copy is already tested there (Step 1); the E2E stamps `GrantedNamespaces` directly and relies on that upstream coverage.

---

## Task 1: New-model memo manifest + API (`api.ts`, `ozmux.toml`, `package.json`)

**Files:**
- Create: `extensions/memo/api.ts`, `extensions/memo/ozmux.toml`
- Modify: `extensions/memo/package.json`
- Test: `src/extension_manager.rs` (inline `#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing tests**

Add these two tests to the `mod tests` block in `src/extension_manager.rs` (the module already has `use super::*;`, which brings in `PathBuf`; add `ExtensionManifest` to the file's top-level `use ozmux_extension_host::{…}` list — see Step 3):

```rust
#[test]
fn bundled_memo_manifest_publishes_memo_main_with_fs_capability() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("extensions/memo/ozmux.toml");
    let toml = std::fs::read_to_string(&path).expect("memo ozmux.toml exists");
    let m = ExtensionManifest::parse(&toml).expect("memo ozmux.toml parses");
    assert_eq!(m.api, vec![PathBuf::from("api.ts")], "memo declares api.ts");
    assert_eq!(m.views.len(), 1, "memo publishes exactly one view");
    let v = &m.views[0];
    assert_eq!(v.id.as_str(), "memo.main");
    assert_eq!(v.entry, PathBuf::from("index.html"));
    assert_eq!(v.capabilities, vec!["fs".to_string()]);
    assert!(v.interactive, "memo.main is interactive");
}

#[test]
fn legacy_discovery_skips_new_model_memo() {
    let bundled = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("extensions");
    let found = discover_command_extensions(&[bundled]);
    assert!(
        !found.iter().any(|d| d.config.name == "memo"),
        "memo's package.json has no name (new-model); legacy discovery must skip it"
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p ozmux-gui --bin ozmux-gui -- --test-threads=1 memo`
Expected: `bundled_memo_manifest_publishes_memo_main_with_fs_capability` FAILS (`memo ozmux.toml exists` — file absent), and `legacy_discovery_skips_new_model_memo` FAILS (current `package.json` still has `"name":"memo"`, so legacy discovery finds it). (`ExtensionManifest` import is added in Step 3; if you run before that, expect a compile error instead — that also counts as "not passing".)

- [ ] **Step 3: Add the `ExtensionManifest` import**

In `src/extension_manager.rs`, extend the existing host-crate import block (currently `use ozmux_extension_host::{ BuiltHostManifest, CommandExtension, … };`) to include `ExtensionManifest`:

```rust
use ozmux_extension_host::{
    BuiltHostManifest, CommandExtension, CommandExtensionConfig, ExtensionControlSet,
    ExtensionManifest, HostProcess, HostRpcClient, Manifest, RegisteredView, ViewId, ViewRegistry,
    apply_control_request, discover_extensions,
};
```

(Keep imports as one contiguous block, alphabetical-ish, per `.claude/rules/rust.md`.)

- [ ] **Step 4: Create `extensions/memo/api.ts`**

```ts
import { readFile } from 'node:fs/promises';

/**
 * memo's host API. The single `fs` namespace exposes file reads to the mounted
 * webview; the view's `capabilities = ["fs"]` grant (in `ozmux.toml`) is what
 * lets `window.fs.read(...)` reach this code. Erasable TS only (Node native
 * type-stripping): no `enum` / parameter-properties / `namespace`.
 */
export default {
  fs: {
    read: async (path: string): Promise<Uint8Array> => await readFile(path),
  },
};
```

- [ ] **Step 5: Create `extensions/memo/ozmux.toml`**

```toml
api = ["api.ts"]

[[views]]
id = "memo.main"
entry = "index.html"
capabilities = ["fs"]
interactive = true
```

- [ ] **Step 6: Replace `extensions/memo/package.json`**

Overwrite the file with exactly:

```json
{
  "type": "module"
}
```

(No `name` → legacy `discover_command_extensions` skips memo. `type:"module"` → Node loads `api.ts` as ESM. This is intentional for the Step 6 coexistence window; Step 5 deletes legacy discovery, and a startup `failed to parse extension package.json` warning for memo during this window is benign — it never reaches the test paths, which call `discover_extensions`/`discover_command_extensions` directly.)

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test -p ozmux-gui --bin ozmux-gui -- --test-threads=1 memo`
Expected: both new tests PASS.

- [ ] **Step 8: Commit**

```bash
git add extensions/memo/api.ts extensions/memo/ozmux.toml extensions/memo/package.json src/extension_manager.rs
git commit -m "feat(memo): new-model api.ts + ozmux.toml manifest (host-API)"
```

---

## Task 2: Rewrite `index.html` to call `window.fs.read`

**Files:**
- Modify: `extensions/memo/index.html`
- Verification: manual (`cargo run --features debug`) — headless tests cannot execute the injected webview JS, so this task has no automated assertion. The complete file content is given below; the rendering path (Proxy bridge injection, MIME, asset serving) is already covered by `extension_render.rs` / `scheme.rs` tests.

- [ ] **Step 1: Replace the file body**

Overwrite `extensions/memo/index.html` with:

```html
<!DOCTYPE html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>memo</title>
    <style>
      body {
        font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
        margin: 0;
        padding: 1rem;
        background: #1a1b26;
        color: #c0caf5;
      }
      input, button {
        font: inherit;
        padding: 0.25rem 0.5rem;
        background: #292e42;
        color: #c0caf5;
        border: 1px solid #414868;
      }
      button[disabled] { opacity: 0.5; }
      #out { margin-top: 1rem; white-space: pre-wrap; }
      .err { color: #f7768e; }
    </style>
  </head>
  <body>
    <h1>Memo</h1>
    <section>
      <p>host-API demo: read a file through <code>window.fs.read</code></p>
      <input id="path" type="text" placeholder="/absolute/path/to/file" size="40" />
      <button id="read" type="button">Read</button>
      <div id="out">(enter a path and press Read)</div>
    </section>

    <script type="module">
      const readBtn = document.getElementById("read");
      const out = document.getElementById("out");

      readBtn.addEventListener("click", async () => {
        const path = document.getElementById("path").value;
        out.classList.remove("err");
        out.textContent = "reading…";
        try {
          // window.fs is a Proxy injected per granted namespace by the host
          // bridge; read() returns a Uint8Array decoded from the {__u8} envelope.
          const bytes = await window.fs.read(path);
          out.textContent = new TextDecoder().decode(bytes);
        } catch (e) {
          out.classList.add("err");
          out.textContent = `error: ${e.message}`;
        }
      });
    </script>
  </body>
</html>
```

- [ ] **Step 2: Manual verification (note for the executor)**

Real rendering can be checked later with `cargo run --features debug` once an extension emits `OSC 5379;mount;memo.main` (e.g. `printf '\e]5379;mount;memo.main\a'` from a pane), then DevTools at `127.0.0.1:9222`. This is out of band for CI; the automated proof of the `fs.read` path is Task 4.

- [ ] **Step 3: Commit**

```bash
git add extensions/memo/index.html
git commit -m "feat(memo): index.html calls window.fs.read (new-model demo)"
```

---

## Task 3: Delete memo's legacy files + remove the legacy memo spawn test

**Files:**
- Delete: `extensions/memo/bootstrap.ts`
- Modify (or Delete): `extensions/memo/tsconfig.json`
- Modify: `src/extension_manager.rs` (remove the legacy memo spawn test + helpers)

- [ ] **Step 1: Delete the legacy entry point**

```bash
git rm extensions/memo/bootstrap.ts
```

- [ ] **Step 2: Re-point `tsconfig.json` at `api.ts` (so `check-types` covers the new file)**

Replace `extensions/memo/tsconfig.json`'s `include` to target `api.ts` instead of the deleted `bootstrap.ts`:

```json
{
  "compilerOptions": {
    "tsBuildInfoFile": "./node_modules/.tmp/tsconfig.tsbuildinfo",
    "target": "es2023",
    "lib": ["ES2023"],
    "module": "nodenext",
    "moduleResolution": "nodenext",
    "types": ["node"],
    "skipLibCheck": true,
    "allowImportingTsExtensions": true,
    "verbatimModuleSyntax": true,
    "moduleDetection": "force",
    "noEmit": true,
    "strict": true,
    "noUnusedLocals": true,
    "noUnusedParameters": true,
    "erasableSyntaxOnly": true,
    "noFallthroughCasesInSwitch": true
  },
  "include": ["api.ts"]
}
```

(`erasableSyntaxOnly: true` is already set, so `tsc` will flag any non-erasable syntax in `api.ts` — the same constraint Node enforces at load. Keep the file rather than deleting it so memo stays typecheck-covered.)

- [ ] **Step 3: Remove the now-broken legacy memo spawn test + helpers**

In `src/extension_manager.rs`'s `mod tests`, delete these three items (they spawn memo as a *legacy* `node bootstrap.ts` extension, which no longer exists):
- `fn memo_dir() -> PathBuf { … }`
- `fn node_and_memo_available() -> bool { … }`
- `#[test] fn endpoint_stays_unpublished_until_extension_is_ready() { … }`

Leave every other test (`register_views_populates_registry_with_capabilities`, `discovers_dirs_with_package_json`, `dedups_by_name_across_roots_first_wins`, `fixes_main_to_bootstrap_ts`, `clearing_the_host_client_drops_stale_in_flight_correlation`, plus Task 1's two new tests) intact — those use temp dirs, not the bundled memo.

- [ ] **Step 4: Verify the gui crate builds and tests pass**

Run: `cargo test -p ozmux-gui --bin ozmux-gui -- --test-threads=1`
Expected: PASS, with no reference to the removed helpers (a leftover caller would be a compile error). If `cargo` reports an unused-import warning for anything the removed test used, clean it up.

- [ ] **Step 5: Typecheck memo**

Run: `pnpm -C extensions/memo check-types`
Expected: PASS (or, if memo has no `node_modules`, run `pnpm install` once at the repo root first). A non-erasable-syntax error here means `api.ts` violates the Node type-stripping constraint — fix `api.ts` to erasable TS.

- [ ] **Step 6: Commit**

```bash
git add -A extensions/memo src/extension_manager.rs
git commit -m "refactor(memo): drop legacy bootstrap.ts + legacy-spawn test (new-model only)"
```

---

## Task 4: E2E — real host loads memo `api.ts`, `fs.read` round-trips, capability gate rejects

**Files:**
- Test: `src/extension_render.rs` (inline `#[cfg(test)] mod tests`, appended after the existing host-call tests)

This is the headline of Step 6: a **real `node` host** loads memo's real `api.ts`, and a capability-gated `fs.read` returns the actual file bytes to the calling webview. The webview's JS layer cannot run headless, so the test injects the `host.call` frame directly onto a `GrantedNamespaces` entity (exactly as the Step 4 tests do) — the difference is the host is real, not a fake echo socket.

- [ ] **Step 1: Write the E2E test**

Append to the `mod tests` block in `src/extension_render.rs` (it already has `use super::*;`):

```rust
fn node_available() -> bool {
    std::process::Command::new("sh")
        .arg("-c")
        .arg("command -v node")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Standard-alphabet base64 of `bytes` (no deps; `base64` is only transitive).
/// Used to assert the `{__u8}` envelope the host returns for a binary value.
fn base64_standard(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        out.push(T[(b0 >> 2) as usize] as char);
        out.push(T[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
        out.push(if chunk.len() > 1 {
            T[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[(b2 & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[test]
fn e2e_memo_fs_read_round_trips_through_the_real_host_and_gates_capabilities() {
    use ozmux_extension_host::host::{LifecycleEvent, RuntimeRoot};
    use ozmux_extension_host::{BuiltHostManifest, HostProcess, HostRpcClient, discover_extensions};
    use std::time::{Duration, Instant};

    if !node_available() {
        eprintln!("skipping e2e: node not available");
        return;
    }

    // 1. Discover the bundled memo and build the host descriptor JSON.
    let extensions_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("extensions");
    let extensions = discover_extensions(&[extensions_root]);
    assert!(
        extensions.iter().any(|e| e.name == "memo"),
        "bundled memo must be discovered via ozmux.toml"
    );
    let built = BuiltHostManifest::new(&extensions);
    let descriptor_json = serde_json::to_string(&built.manifest).expect("manifest serializes");

    // 2. Spawn the real Node host and wait for readiness.
    let runtime = RuntimeRoot::resolve_in(&std::env::temp_dir(), std::process::id(), "host-e2e")
        .expect("runtime root");
    let host = HostProcess::spawn(runtime, &descriptor_json, Duration::from_secs(20))
        .expect("spawn host");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match host.events().recv_timeout(Duration::from_millis(200)) {
            Ok(LifecycleEvent::Ready) => break,
            Ok(LifecycleEvent::SpawnFailed { error }) => panic!("host spawn failed: {error}"),
            Ok(LifecycleEvent::Exited { status }) => panic!("host exited early: {status:?}"),
            Err(_) => assert!(Instant::now() < deadline, "host never became ready"),
        }
    }
    let client = HostRpcClient::connect(host.rpc_sock_path()).expect("rpc connect");

    // 3. Headless app with the capability gate + reply drain + a real client.
    let mut app = gate_app();
    app.add_systems(Update, drain_host_rpc_responses);
    app.world_mut().resource_mut::<HostRpc>().set_client(client);

    // 4. A webview granted the "fs" namespace (the trust record on the entity).
    let mut caps = std::collections::HashSet::new();
    caps.insert("fs".to_string());
    let webview = app.world_mut().spawn(GrantedNamespaces(caps)).id();

    // 5. fs.read of a known temp file.
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("note.txt");
    let content = b"hello from memo fs.read";
    std::fs::write(&file, content).unwrap();

    app.world_mut().trigger(Receive {
        webview,
        payload: OzmuxFrame(serde_json::json!({
            "kind": "host.call", "reqId": "p0", "ns": "fs", "method": "read",
            "args": [file.to_string_lossy()]
        })),
    });

    // 6. Pump until the reply lands.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        app.update();
        if !app.world().resource::<CapturedEmits>().0.is_empty() {
            break;
        }
        assert!(Instant::now() < deadline, "fs.read reply never returned");
        std::thread::sleep(Duration::from_millis(10));
    }
    let cap = app.world().resource::<CapturedEmits>();
    assert_eq!(cap.0.len(), 1, "exactly one reply");
    let (target, payload) = &cap.0[0];
    assert_eq!(*target, webview, "reply targets the originating webview");
    let reply: serde_json::Value = serde_json::from_str(payload).unwrap();
    assert_eq!(reply["reqId"], "p0", "page-local reqId restored");
    assert_eq!(reply["ok"], true, "fs.read succeeded: {payload}");
    assert_eq!(
        reply["value"]["__u8"].as_str().expect("binary {__u8} envelope"),
        base64_standard(content),
        "fs.read returns the file's bytes as a base64 envelope"
    );

    // 7. Capability gate: an ungranted namespace is rejected, host not called.
    app.world_mut().resource_mut::<CapturedEmits>().0.clear();
    app.world_mut().trigger(Receive {
        webview,
        payload: OzmuxFrame(serde_json::json!({
            "kind": "host.call", "reqId": "p1", "ns": "net", "method": "get", "args": []
        })),
    });
    app.world_mut().flush();
    let cap = app.world().resource::<CapturedEmits>();
    assert_eq!(cap.0.len(), 1, "exactly one reject");
    assert!(
        cap.0[0].1.contains("capability_denied"),
        "an ungranted namespace must be rejected, not forwarded"
    );

    app.world_mut().resource_mut::<HostRpc>().clear_client();
    drop(host);
}
```

- [ ] **Step 2: Run the E2E test**

Run: `cargo test -p ozmux-gui --bin ozmux-gui -- --test-threads=1 e2e_memo_fs_read`
Expected: PASS (or a `skipping e2e: node not available` line if `node` is absent — acceptable in that environment).

- [ ] **Step 3: Contingency if the host fails to load `api.ts`**

If the test panics at `host never became ready` / `host exited early`, the host could not `import()` memo's `api.ts`. Diagnose by running the host directly against the same descriptor and reading stderr:

```bash
# Reproduce the descriptor the test builds, then run the host by hand:
RUST_LOG=debug cargo test -p ozmux-gui --bin ozmux-gui -- --test-threads=1 e2e_memo_fs_read --nocapture
```

Most likely causes and fixes:
- **Non-erasable syntax in `api.ts`** → `ERR_UNSUPPORTED_TYPESCRIPT_SYNTAX`. Fix `api.ts` to erasable TS (Task 1).
- **Absolute-path `import()` rejected** (`ERR_UNSUPPORTED_ESM_URL_SCHEME`/`ERR_MODULE_NOT_FOUND` on the absolute `apiPaths`). Fix in `host/src/main.ts`: wrap the importer with `pathToFileURL`:
  ```ts
  import { pathToFileURL } from 'node:url';
  // …
  const { api, warnings } = await loadHostApi(manifest.extensions, (s) => import(pathToFileURL(s).href));
  ```
  Then rebuild the embedded bundle: `pnpm -C host build` (regenerates `assets/host.mjs`), and re-run. This is the **only** scenario in which Step 6 touches `host/src` / `assets/host.mjs`; if you hit it, add `host/src/main.ts` + `assets/host.mjs` to the Task 4 commit.

- [ ] **Step 4: Commit**

```bash
git add src/extension_render.rs
git commit -m "test(memo): e2e fs.read round-trips through the real host + capability gate"
```

---

## Task 5: Full verification + cleanup

**Files:** none (verification only)

- [ ] **Step 1: Run every affected suite**

```bash
cargo test -p ozmux_extension_host
cargo test -p ozmux-gui --bin ozmux-gui -- --test-threads=1
pnpm -C host test
pnpm -C extensions/memo check-types
```
Expected: all PASS (the gui-crate E2E may print the skip line if `node` is absent).

- [ ] **Step 2: Confirm the host bundle is unchanged (unless the Task 4 contingency fired)**

Run: `git status --short assets/host.mjs host/`
Expected: **empty** — Step 6 must not modify `assets/host.mjs` or `host/src` unless the Task 4 absolute-path contingency was needed. If non-empty without that contingency, you rebuilt the bundle unnecessarily — revert it.

- [ ] **Step 3: Build, lint, format**

```bash
cargo build
cargo clippy --workspace --all-targets
cargo fmt --check
pnpm lint
```
Expected: clean. (`pnpm lint` runs biome over `extensions/**`, covering memo's `api.ts`/`index.html`.)

- [ ] **Step 4: Sanity-check the legacy surface still works during the coexistence window**

memo is now new-model; `browser`/`md` remain legacy until Step 5. Confirm nothing else regressed:
Run: `cargo test -p ozmux-gui --bin ozmux-gui -- --test-threads=1`
Expected: PASS. (No legacy infrastructure was removed in Step 6 — only memo's own legacy files.)

---

## Self-Review

**1. Spec coverage** (against §4④「実装ステップ順序・移行スコープ」 + §4④移行 + §5 テスト戦略):
- "memo を新モデルへ全面置換 (`api.ts`/`ozmux.toml`/`index.html`)" → Tasks 1, 2. ✓
- "Step 6 で memo のレガシーファイルを削除 (`bootstrap.ts`/`package.json`/`tsconfig.json`)" → Task 1 (package.json replaced), Task 3 (bootstrap.ts deleted, tsconfig re-pointed). The plan **keeps** a minimal `package.json`/`tsconfig.json` rather than deleting outright — `package.json:{"type":"module"}` is load-bearing for ESM + legacy-skip (Key facts), and `tsconfig.json` keeps `check-types` coverage; this satisfies the spec's intent (stop legacy double-registration) while staying correct. Called out explicitly here so it is not read as a deviation. ✓
- "E2E: host 起動 → window.fs.read → 期待バイト → 未許可 namespace は reject" → Task 4 (real host, `fs.read` bytes asserted via base64 envelope, `capability_denied` for ungranted ns). The OSC-mount trigger and the webview JS layer are covered upstream (osc_webview.rs Step-1 tests; host_bridge injection Step-4 tests) — noted under "Untouched". ✓
- "既存 extension_render ハーネスを再利用" → Task 4 uses `gate_app`/`CapturedEmits`. ✓
- "`--test-threads=1`" → every gui-crate run uses it. ✓
- Host runtime tests (loader/dispatch/codec) already exist generically (`pnpm -C host test`, 37 cases); Step 6 adds no host-side test because it adds no host-side code — the memo-specific proof is the Rust E2E. ✓

**2. Placeholder scan:** no TBD/TODO/"handle errors"/"similar to". Every code step shows full content. ✓

**3. Type consistency:** `discover_extensions` → `Vec<DiscoveredExtension>` (`.name`); `BuiltHostManifest::new(&[DiscoveredExtension]) -> BuiltHostManifest` (`.manifest`); `HostProcess::spawn(RuntimeRoot, &str, Duration)`; `host.events(): &Receiver<LifecycleEvent>`; `host.rpc_sock_path(): &Path`; `HostRpcClient::connect(&Path)`; `HostRpc::set_client/clear_client`; `GrantedNamespaces(HashSet<String>)`; `Receive { webview, payload }`; `OzmuxFrame(serde_json::Value)`; `CapturedEmits(Vec<(Entity, String)>)`; `ExtensionManifest::parse(&str) -> ExtensionResult<ExtensionManifest>` with `.api: Vec<PathBuf>` and `.views[].{id: ViewId, entry: PathBuf, capabilities: Vec<String>, interactive: bool}` — all match the verified signatures. The `"fs"` namespace / `"memo.main"` view id / `["fs"]` capability are consistent across `api.ts`, `ozmux.toml`, and both tests. ✓
