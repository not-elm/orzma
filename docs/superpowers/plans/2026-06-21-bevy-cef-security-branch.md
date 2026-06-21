# bevy_cef security branch switch — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move ozmux's `bevy_cef` / `bevy_cef_core` dependency from the `passthrough` branch to the `security` branch (PR #61), removing the inherited `disable-web-security` / `ignore-certificate-errors` / `ignore-ssl-errors` / `allow-running-insecure-content` switches and making CEF secure-by-default.

**Architecture:** Pure dependency-metadata change in the base case — flip four `branch = "passthrough"` strings to `"security"` and re-pin `Cargo.lock`. Verify the secure default does not break ozmux's webview (static audit + manual runtime check). Source changes occur only as a contingency (CORS headers on the `ozma-dyn://` scheme) if verification surfaces a genuine cross-origin dependency.

**Tech Stack:** Rust 2024 / Cargo (git deps + lockfile pinning), `just` task runner, `bevy_cef` (CEF v145), ozmux `ozma-dyn://` custom scheme.

## Global Constraints

- **Branch label in `Cargo.toml`, exact sha in `Cargo.lock`** — keep the repo's existing convention (human-readable `branch = "…"` in manifests; the precise commit is pinned by the lockfile).
- **Never re-enable `disable-web-security`** — if a cross-origin need appears, fix forward by emitting `Access-Control-*` headers from the `ozma-dyn://` handler, not by opting back into the risky switch.
- **Leave the sandbox at `SandboxMode::PlatformDefault`** — do not set `CefPlugin::sandbox` (the macOS render process is not yet linked against `cef_sandbox`; `Enabled` would only `warn!`). `PlatformDefault` is documented as "no behavior change".
- **`docs/` is gitignored but force-added by convention** — use `git add -f` for any file under `docs/`.
- **Rust rules apply to any source change** (comment taxonomy `// TODO:` / `// NOTE:` / `// SAFETY:` only; doc comments on `pub` items; imports at top; no `mod.rs`) — relevant only to the contingency task.
- **Commit messages** end with: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

---

### Task 1: Switch the bevy_cef dependency to the `security` branch

**Files:**
- Modify: `Cargo.toml:26` (`bevy_cef` git dep)
- Modify: `Cargo.toml:92` (`[workspace.dependencies] bevy_cef_core`)
- Modify: `crates/ozma_webview/Cargo.toml:17` (`bevy_cef` git dep)
- Modify: `justfile:11` (`bevy_cef_branch`)
- Modify (regenerated): `Cargo.lock`

**Interfaces:**
- Consumes: nothing (first task).
- Produces: a workspace that compiles against `bevy_cef` / `bevy_cef_core` at `branch = "security"`, with `Cargo.lock` repinned to the `security` HEAD. The secure-by-default behavior (no risky CEF switches) is now in effect. No new symbols.

- [ ] **Step 1: Flip the four `passthrough` strings to `security`**

There are exactly four occurrences (verified). Apply each edit:

`Cargo.toml:26`
```toml
bevy_cef = { git = "https://github.com/not-elm/bevy_cef", branch = "security" }
```

`Cargo.toml:92`
```toml
bevy_cef_core = { git = "https://github.com/not-elm/bevy_cef", branch = "security" }
```

`crates/ozma_webview/Cargo.toml:17`
```toml
bevy_cef = { git = "https://github.com/not-elm/bevy_cef", branch = "security" }
```

`justfile:11`
```just
bevy_cef_branch := "security"
```

- [ ] **Step 2: Verify no `passthrough` reference remains in manifests/justfile**

Run: `grep -rn 'passthrough' Cargo.toml crates/ozma_webview/Cargo.toml justfile`
Expected: no output (exit code 1).

- [ ] **Step 3: Re-pin the lockfile to the `security` branch HEAD**

Run: `cargo update -p bevy_cef -p bevy_cef_core`
Expected: cargo reports updating `bevy_cef` and `bevy_cef_core` to a new git commit (the `security` HEAD, currently `552dbd0`).

If cargo errors that the package source changed and cannot be updated in place, run `cargo build` instead — it re-resolves and rewrites `Cargo.lock` from the new manifest source. Either path must leave the lockfile pointing at `?branch=security`.

- [ ] **Step 4: Verify the lockfile now pins the `security` branch**

Run: `grep -A2 'name = "bevy_cef"' Cargo.lock | grep source; grep -A2 'name = "bevy_cef_core"' Cargo.lock | grep source`
Expected: both `source` lines read `source = "git+https://github.com/not-elm/bevy_cef?branch=security#<sha>"` (no `passthrough`).

- [ ] **Step 5: Build the workspace**

Run: `cargo build`
Expected: compiles successfully. (First build re-fetches the `security` branch; allow time for the CEF crates to rebuild.)

- [ ] **Step 6: Run the workspace test suite**

Run: `cargo test --workspace`
Expected: PASS. ozmux's own tests are unaffected by the switch; the upstream `bevy_cef_core` security unit tests (`risky_present`, `effective_command_line_config`, `resolve_no_sandbox`) also run and pass.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml crates/ozma_webview/Cargo.toml justfile Cargo.lock
git commit -m "build(deps): switch bevy_cef to security branch (PR #61)

Removes the inherited disable-web-security / ignore-certificate-errors /
ignore-ssl-errors / allow-running-insecure-content switches; CEF is now
secure-by-default. Sandbox stays at PlatformDefault (no behavior change).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Static audit — confirm the secure default holds

**Files:**
- Read-only audit (no file changes): `crates/ozma_webview/src/webview/render/ozma_bridge.js`, `sdk/ozma-web/src/`, `crates/webview_host/src/dyn_scheme.rs`, `crates/ozma_webview/src/webview/`.

**Interfaces:**
- Consumes: the switched dependency from Task 1.
- Produces: a go/no-go verdict for the contingency. **Clean verdict** → Task 3 (runtime check) proceeds and Contingency C1 is skipped. **Cross-origin found** → Contingency C1 runs.

- [ ] **Step 1: Confirm the injected bridge and SDK use no HTTP/cross-origin**

Run: `grep -rn "fetch(\|XMLHttpRequest\|http://\|https://\|Access-Control\|crossorigin" crates/ozma_webview/src/webview/render/ozma_bridge.js sdk/ozma-web/src/`
Expected: no output. The `window.ozma` bridge is pure CEF process-message IPC (`cef.emit` → `Receive<OzmuxFrame>`); the Same-Origin Policy does not touch it.

- [ ] **Step 2: Confirm the `ozma-dyn://` scheme serves same-origin assets**

Run: `grep -n "CefSchemeOptions::\|headers" crates/webview_host/src/dyn_scheme.rs`
Expected: the scheme is registered `STANDARD | SECURE | CORS_ENABLED | FETCH_ENABLED | DISPLAY_ISOLATED`, and responses set `headers: Vec::new()`. Each handle is its own origin (`ozma-dyn://<handle>/…`), so asset loads are same-origin and need no relaxed SOP.

- [ ] **Step 3: Record the verdict**

State the conclusion explicitly in the task report:
- **No cross-origin usage found** (expected): the secure default holds for `ozma-dyn://`; remote `Webview::url(...)` pages now correctly enforce SOP + TLS (a proper-browser improvement, not a regression). Proceed to Task 3; skip Contingency C1.
- **Cross-origin usage found:** name the exact file/line and the origin pair; execute Contingency C1.

No commit (audit only).

---

### Task 3: Manual runtime verification (macOS GUI — requires the user)

**Files:** none (observation only).

**Interfaces:**
- Consumes: the built binary from Task 1 and the clean verdict from Task 2.
- Produces: human confirmation that webview rendering, `ozma-dyn://` asset loading, and the `window.ozma` bridge still work with the secure default, plus confirmation that the risky-switch `warn!` does not fire.

> This task cannot be performed by a subagent — it needs a display and a human observer. The orchestrator hands this to the user to run.

- [ ] **Step 1: Launch ozmux with logging**

Run: `RUST_LOG=info,bevy_cef=warn cargo run --features debug`
Expected: the app window opens; a `tmux -CC` session attaches.

- [ ] **Step 2: Mount a dynamic (`ozma-dyn://`) webview and a remote page**

Inside the running session, mount a webview. Easiest paths:
- Dynamic asset / bridge: run the `sdk/ratatui-ozma` flow that registers inline/dynamic content and exercises `window.ozma.call`/`on`.
- Remote page: run the `ratatui_remote_url` example (`sdk/ratatui-ozma/examples/ratatui_remote_url.rs`, mounts `https://github.com/...`) or `apps/ozbrowser`.

- [ ] **Step 3: Observe and confirm**

Confirm all of:
1. The mounted page renders.
2. `ozma-dyn://` assets load (no blank/404 page for dynamic content).
3. The `window.ozma.call` / `on` round-trip works (the registering program receives the call and the page receives the response/event).
4. A remote `https://` page loads and renders (SOP/TLS now enforced — valid sites still work).

- [ ] **Step 4: Confirm no risky switch is active**

In the captured logs, confirm the new one-time bevy_cef startup `warn!` about risk-relaxing switches **does not appear**. Its absence is positive proof that `disable-web-security` and friends are off.

- [ ] **Step 5: Record the result**

If all confirmations pass → the switch is complete; close out. If any webview feature genuinely fails due to a cross-origin block → execute Contingency C1 and re-run this task.

---

### Contingency C1: CORS headers on the `ozma-dyn://` handler (run ONLY if Task 2 or Task 3 finds a real cross-origin block)

**Files:**
- Modify: `crates/webview_host/src/dyn_scheme.rs` (the `OzmuxDynScheme::handle` responses + a unit test in the existing `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: the failing cross-origin scenario identified in Task 2/3 (exact origin pair).
- Produces: `ozma-dyn://` responses that carry `Access-Control-Allow-Origin`, mirroring how the upstream `cef://localhost/` scheme sends permissive CORS headers — without disabling web security.

> Do not run this task unless verification surfaced a concrete cross-origin failure. The static audit (Task 2) is expected to come back clean, in which case this task is skipped.

- [ ] **Step 1: Write the failing test**

In the `#[cfg(test)] mod tests` block of `crates/webview_host/src/dyn_scheme.rs`, assert the served responses carry a CORS header. Use the existing test helpers/fixtures in that module (e.g. the registry + `resolve_request`/handler path already used by the surrounding tests) to build a served response, then:

```rust
#[test]
fn served_dyn_response_carries_cors_header() {
    let reg = sample_registry_with_inline("i1");
    let resp = serve_via_scheme(&reg, "ozma-dyn://i1/index.html");
    assert!(
        resp.headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("Access-Control-Allow-Origin") && v == "*"),
        "ozma-dyn responses must advertise CORS to match cef://localhost/ behavior"
    );
}
```

Adapt `sample_registry_with_inline` / `serve_via_scheme` to the exact fixture and entry point already present in the module (match the names used by neighboring tests).

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p ozmux_webview_host --features cef served_dyn_response_carries_cors_header`
Expected: FAIL (responses currently set `headers: Vec::new()`).

- [ ] **Step 3: Add the CORS header to served responses**

In `OzmuxDynScheme::handle`, replace the empty `headers: Vec::new()` on the inline-HTML and static-asset success arms with a shared header set:

```rust
fn cors_headers() -> Vec<(String, String)> {
    vec![("Access-Control-Allow-Origin".to_string(), "*".to_string())]
}
```

Use `headers: cors_headers()` on the success responses. Place the helper as a private `fn` after the trait `impl` (private items last). Keep error responses (`not_found`, `status_text`) as-is unless the failing scenario requires CORS on errors too.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p ozmux_webview_host --features cef served_dyn_response_carries_cors_header`
Expected: PASS.

- [ ] **Step 5: Run the crate's full suite + lints**

Run: `cargo test -p ozmux_webview_host --features cef && cargo clippy -p ozmux_webview_host --features cef --all-targets`
Expected: PASS, clippy clean.

- [ ] **Step 6: Commit**

```bash
git add crates/webview_host/src/dyn_scheme.rs
git commit -m "fix(webview): send CORS headers from ozma-dyn handler

Fixes a cross-origin block surfaced after dropping disable-web-security;
mirrors the upstream cef://localhost/ CORS behavior instead of re-enabling
the risky switch.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Notes for the executor

- **Release bundling** (out of band): when next bundling the macOS `.app`, re-run `just setup-cef-release` so the release render process is rebuilt from the `security` branch (the `justfile:11` bump in Task 1 wires this). Dev runs (`cargo run --features debug`) use the crates.io debug render process and need no extra step.
- **After PR #61 merges to `main`:** re-point the four `branch = "security"` strings to `"main"` and re-run `cargo update` (post-merge `main ⊇ security`). One-line follow-up; not part of this plan.
