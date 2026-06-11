# Phase 1 — Step 1: Capability & Manifest Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Introduce the Rust capability/trust data model — `ozmux.toml` extension-manifest parsing, a capability-aware `ViewRegistry`, and a `GrantedNamespaces` component stamped onto OSC-mounted surfaces — without changing the running process model or removing anything.

**Architecture:** A new `extension_manifest` module parses `ozmux.toml` into views (`id` / `entry` / `capabilities` / `interactive`). `RegisteredView` gains a `capabilities` field. The existing OSC-mount observer copies a view's capabilities onto the new surface entity as a `GrantedNamespaces(HashSet<String>)` component, which the later host-API bridge (Step 3) will read as the per-surface capability grant. This step is **purely additive** — the app still runs the old per-extension model and all existing tests keep passing.

**Tech Stack:** Rust 2024, Bevy 0.18 ECS, `toml`, `serde`, `thiserror`.

---

## Phase 1 plan sequence (this is Step 1 of 5)

Each step is its own plan document, authored just-in-time against the then-current code so it never goes stale:

1. **Step 1 (this doc) — Capability & manifest foundation** (Rust, additive): `extension_manifest` parser, `RegisteredView.capabilities`, `GrantedNamespaces` at mount.
2. **Step 2 — Single host process + extension loader**: `@ozmux/host` runtime (loads `extensions/*/api.ts`, RPC dispatch, asset server); reshape `ExtensionManagerPlugin` to spawn exactly one `node` host; scan extension roots, parse `ozmux.toml`, populate `ViewRegistry` with capabilities; extend the asset `Request` to `{extension, path}`; config extension roots (user-first).
3. **Step 3 — Host-API bridge**: Proxy preload injection (replace `ozmux.js`), single-object `cef.emit`, Rust capability gate keyed on `Receive<_>.webview` Entity + `GrantedNamespaces`, forward to host socket, `reqId→Entity` correlation + `HostEmitEvent` return, base64 `{__u8}` binary codec + max-size guardrail.
4. **Step 4 — Remove old machinery**: delete command shim / handlers / channels / `bootstrap()` / control plane (`register_view`/`split`/`add_surface`/`activate`) / `handlers_bridge`; remove `window.ozmux.call/subscribe`; drop SDK `./server` + `./cmd-shim`.
5. **Step 5 — memo migration + `extensions/*` root**: convert `@memo` to `extensions/memo` (`api.ts` + `ozmux.toml` + `index.html`); add `extensions/*` workspace + discovery root (user-first); E2E.

Spec: `docs/superpowers/specs/2026-06-11-phase1-single-host-process-design.md`.

> **Note on running binary-crate tests:** the repo has a known pre-existing IME-test failure and a parallel-teardown SIGSEGV (see project memory). When a step runs the root binary's tests, use `cargo test <filter> -- --test-threads=1` for a clean signal. The library-crate tests in Task 1 & 2 are unaffected.

---

## File Structure

| File | Responsibility | Action |
| --- | --- | --- |
| `crates/extension_host/src/extension_manifest.rs` | Parse `ozmux.toml` → `ExtensionManifest { views: Vec<ExtensionView> }` | Create |
| `crates/extension_host/src/lib.rs` | Declare + re-export `extension_manifest` | Modify |
| `crates/extension_host/Cargo.toml` | Add `toml` dependency | Modify |
| `crates/extension_host/src/registry.rs` | Add `capabilities` to `RegisteredView` | Modify |
| `crates/extension_host/src/bridge.rs` | Update `handle_register_view` literal | Modify |
| `src/osc_webview.rs` | Add `GrantedNamespaces`; stamp it at mount | Modify |

---

## Task 1: `ozmux.toml` extension manifest parser

**Files:**
- Create: `crates/extension_host/src/extension_manifest.rs`
- Modify: `crates/extension_host/Cargo.toml`
- Modify: `crates/extension_host/src/lib.rs:11` (module declarations) and `:28` (re-exports)
- Test: inline `#[cfg(test)] mod tests` in `extension_manifest.rs`

- [ ] **Step 1: Add the `toml` dependency**

In `crates/extension_host/Cargo.toml`, under `[dependencies]` (after the `serde_json` line), add:

```toml
toml = { workspace = true }
```

- [ ] **Step 2: Create the parser file with a failing test**

Create `crates/extension_host/src/extension_manifest.rs` with the test module only first (the types it references do not exist yet, so it will fail to compile):

```rust
//! Parses an extension's `ozmux.toml` manifest: the views it publishes for OSC
//! mounting and the host-API capabilities each view is granted.

use serde::Deserialize;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_view_with_capabilities() {
        let text = r#"
[[views]]
id = "memo.main"
entry = "index.html"
capabilities = ["fs"]
interactive = true
"#;
        let m = ExtensionManifest::parse(text).unwrap();
        assert_eq!(m.views.len(), 1);
        let v = &m.views[0];
        assert_eq!(v.id, "memo.main");
        assert_eq!(v.entry, "index.html");
        assert_eq!(v.capabilities, vec!["fs".to_string()]);
        assert!(v.interactive);
    }

    #[test]
    fn capabilities_and_interactive_default_to_empty_and_false() {
        let text = r#"
[[views]]
id = "v"
entry = "a.html"
"#;
        let v = &ExtensionManifest::parse(text).unwrap().views[0];
        assert!(v.capabilities.is_empty());
        assert!(!v.interactive);
    }

    #[test]
    fn empty_text_has_no_views() {
        assert!(ExtensionManifest::parse("").unwrap().views.is_empty());
    }

    #[test]
    fn missing_required_field_errors() {
        let text = r#"
[[views]]
entry = "a.html"
"#;
        assert!(matches!(
            ExtensionManifest::parse(text),
            Err(ExtensionManifestError::Toml(_))
        ));
    }

    #[test]
    fn rejects_malformed_toml() {
        assert!(matches!(
            ExtensionManifest::parse("[[views]"),
            Err(ExtensionManifestError::Toml(_))
        ));
    }
}
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test -p ozmux_extension_host extension_manifest`
Expected: compile error — `cannot find type ExtensionManifest`/`ExtensionManifestError` in this scope.

- [ ] **Step 4: Implement the parser**

Insert this implementation **above** the `#[cfg(test)] mod tests` block (immediately after the `use serde::Deserialize;` line):

```rust
/// An extension's resolved manifest: the views it publishes for OSC mounting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionManifest {
    /// Views this extension publishes, addressable by `view_id` from OSC mounts.
    pub views: Vec<ExtensionView>,
}

/// One view an extension publishes for OSC mounting, with its capability grant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionView {
    /// PTY-facing identifier referenced by `OSC mount;<id>`.
    pub id: String,
    /// HTML entry path relative to the extension dir (e.g. `index.html`).
    pub entry: String,
    /// Host-API namespaces this view's webview may call (namespace granularity).
    pub capabilities: Vec<String>,
    /// Whether the mounted webview accepts pointer/keyboard input.
    pub interactive: bool,
}

impl ExtensionManifest {
    /// Parses an `ozmux.toml` string into a `ExtensionManifest`.
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
        Ok(Self { views })
    }
}

/// A failure to parse an extension manifest.
#[derive(Debug, thiserror::Error)]
pub enum ExtensionManifestError {
    /// Malformed or invalid `ozmux.toml`.
    #[error("invalid ozmux.toml: {0}")]
    Toml(#[source] toml::de::Error),
}

#[derive(Deserialize)]
struct RawManifest {
    #[serde(default)]
    views: Vec<RawView>,
}

#[derive(Deserialize)]
struct RawView {
    id: String,
    entry: String,
    #[serde(default)]
    capabilities: Vec<String>,
    #[serde(default)]
    interactive: bool,
}
```

- [ ] **Step 5: Declare and re-export the module in `lib.rs`**

In `crates/extension_host/src/lib.rs`, add the module declaration in alphabetical order among the existing `pub mod` lines (after `pub mod path_prefix;` at line 12):

```rust
pub mod extension_manifest;
```

And add the re-export after the `manifest` re-export (line 28, `pub use manifest::{Manifest, ManifestError};`):

```rust
pub use extension_manifest::{ExtensionManifest, ExtensionManifestError, ExtensionView};
```

- [ ] **Step 6: Run the test to verify it passes**

Run: `cargo test -p ozmux_extension_host extension_manifest`
Expected: PASS (5 tests).

- [ ] **Step 7: Commit**

```bash
git add crates/extension_host/Cargo.toml crates/extension_host/src/extension_manifest.rs crates/extension_host/src/lib.rs
git commit -m "feat(extension_host): parse ozmux.toml extension manifest with view capabilities"
```

---

## Task 2: Capability-aware `RegisteredView`

**Files:**
- Modify: `crates/extension_host/src/registry.rs` (struct + test)
- Modify: `crates/extension_host/src/bridge.rs:266-270` (construction literal)
- Modify: `src/osc_webview.rs:131-140` (test helper literal)

> Adding a struct field is a compile-time change: the "failing test" here is a **compilation error** until every `RegisteredView { .. }` literal in the workspace includes the new field. There are exactly three literals: `registry.rs` (test), `bridge.rs` (`handle_register_view`), and `src/osc_webview.rs` (test helper).

- [ ] **Step 1: Update the registry test to expect a `capabilities` field**

In `crates/extension_host/src/registry.rs`, replace the `register_then_get_roundtrips` test body (lines 41-53) so it registers and asserts capabilities:

```rust
    #[test]
    fn register_then_get_roundtrips() {
        let mut reg = ViewRegistry::default();
        reg.register(
            "dashboard".into(),
            RegisteredView {
                entry: "dash.html".into(),
                owning_ext: "memo".into(),
                interactive: true,
                capabilities: vec!["fs".into()],
            },
        );
        assert_eq!(reg.get("dashboard").map(|v| v.interactive), Some(true));
        assert_eq!(
            reg.get("dashboard").map(|v| v.capabilities.clone()),
            Some(vec!["fs".to_string()])
        );
        assert!(reg.get("missing").is_none());
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p ozmux_extension_host`
Expected: compile error — `struct RegisteredView has no field named capabilities`.

- [ ] **Step 3: Add the field and update the two non-test literals**

In `crates/extension_host/src/registry.rs`, add the field to `RegisteredView` (after the `interactive` field, line 17):

```rust
    /// Whether the mounted webview accepts pointer/keyboard input.
    pub interactive: bool,
    /// Host-API namespaces a webview mounting this view may call (namespace
    /// granularity). Empty for control-plane registrations (legacy path).
    pub capabilities: Vec<String>,
```

In `crates/extension_host/src/bridge.rs`, update the `handle_register_view` literal (lines 266-270) to set an empty capability list (the legacy control plane carries none):

```rust
        RegisteredView {
            entry: p.entry,
            owning_ext: caller.to_string(),
            interactive: p.interactive,
            capabilities: Vec::new(),
        },
```

In `src/osc_webview.rs`, update the `register_view` test helper literal (lines 134-139) to compile:

```rust
            RegisteredView {
                entry: "ui/dash.html".into(),
                owning_ext: "memo".into(),
                interactive,
                capabilities: Vec::new(),
            },
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p ozmux_extension_host`
Expected: PASS (all extension_host tests, including `register_then_get_roundtrips`).

- [ ] **Step 5: Commit**

```bash
git add crates/extension_host/src/registry.rs crates/extension_host/src/bridge.rs src/osc_webview.rs
git commit -m "feat(extension_host): add capabilities field to RegisteredView"
```

---

## Task 3: Stamp `GrantedNamespaces` on OSC-mounted surfaces

**Files:**
- Modify: `src/osc_webview.rs` (add component, stamp at mount, add a test helper + test)

- [ ] **Step 1: Write the failing test**

In `src/osc_webview.rs`, inside the `#[cfg(test)] mod tests` block, add a capability-aware helper next to the existing `register_view` helper (after line 140):

```rust
    fn register_view_with_caps(app: &mut App, view_id: &str, interactive: bool, caps: &[&str]) {
        app.world_mut().resource_mut::<ViewRegistry>().register(
            view_id.into(),
            RegisteredView {
                entry: "ui/dash.html".into(),
                owning_ext: "memo".into(),
                interactive,
                capabilities: caps.iter().map(|s| (*s).to_string()).collect(),
            },
        );
    }
```

Then add this test at the end of the `mod tests` block (before its closing `}`):

```rust
    #[test]
    fn mount_stamps_granted_namespaces_from_view_capabilities() {
        let mut app = make_test_app();
        register_view_with_caps(&mut app, "dash", true, &["fs"]);

        let (pane, terminal_surface) = app
            .world_mut()
            .run_system_once(|mut mux: MultiplexerCommands| {
                let o = mux.create_workspace(Some("t".into()));
                (o.pane, o.surface)
            })
            .unwrap();
        app.world_mut().flush();

        app.world_mut().trigger(OscWebviewRequest {
            entity: terminal_surface,
            verb: OscWebviewVerb::Mount {
                view_id: "dash".into(),
            },
        });
        app.world_mut().flush();

        let active = active_surface(&app, pane).expect("active surface");
        let granted = app
            .world()
            .get::<GrantedNamespaces>(active)
            .expect("mount must stamp GrantedNamespaces");
        assert!(
            granted.0.contains("fs"),
            "granted namespaces must contain the view's capability"
        );
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test mount_stamps_granted_namespaces_from_view_capabilities -- --test-threads=1`
Expected: compile error — `cannot find type GrantedNamespaces in this scope`.

- [ ] **Step 3: Add the component and stamp it at mount**

In `src/osc_webview.rs`, add the component definition after `NonInteractive` (after line 30):

```rust
/// The host-API namespaces an OSC-mounted webview is permitted to call, copied
/// from its registered view's `capabilities` at mount time. The host-API bridge
/// (Step 3) reads this as the per-surface capability grant; it is the in-ECS
/// trust record, never derived from webview-supplied data.
#[derive(Component, Debug, Clone, Default)]
pub(crate) struct GrantedNamespaces(pub(crate) std::collections::HashSet<String>);
```

In the `Mount` arm of `on_osc_webview_request`, capture the capabilities alongside the other view fields. After the line `let owning = view.owning_ext.clone();` (line 77), add:

```rust
            let capabilities = view.capabilities.clone();
```

Then, after the `OscMounted { .. }` insert block (after line 88, before the `if !interactive` block), stamp the component:

```rust
            mux.insert_on(
                surface,
                GrantedNamespaces(capabilities.into_iter().collect()),
            );
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test mount_stamps_granted_namespaces_from_view_capabilities -- --test-threads=1`
Expected: PASS.

- [ ] **Step 5: Run the full osc_webview suite to confirm no regressions**

Run: `cargo test osc_webview -- --test-threads=1`
Expected: PASS (all OSC-webview tests, including the pre-existing mount/unmount tests).

- [ ] **Step 6: Commit**

```bash
git add src/osc_webview.rs
git commit -m "feat(osc-webview): stamp GrantedNamespaces on mount from view capabilities"
```

---

## Done criteria for Step 1

- `cargo test -p ozmux_extension_host` passes (manifest parser + capability-aware registry).
- `cargo test osc_webview -- --test-threads=1` passes (GrantedNamespaces stamped at mount).
- No existing behavior removed; the app still builds and runs the legacy per-extension model.
- `cargo clippy --workspace` and `cargo fmt` clean (run `make fix-lint` before the final commit if needed).

After this step lands, Step 2 (single host process + extension loader) will be authored against the updated tree.

## Carry into Step 2 (from Step 1 final integration review)

- **Validate `entry` for path traversal before it reaches `SurfaceKind::Extension`.** Step 2 is where parsed `ozmux.toml` `entry` values first flow into the multiplexer. Add a "non-empty, no `..` component" check in the manifest loader (or at registry insertion) to close the directory-traversal window before Step 3 opens the API bridge.
- **Validate `id` format/uniqueness.** The registry is indexed by `id`; require non-empty, whitespace-free (recommended `<extension>.<view>` shape) so `OSC mount;<view_id>` stays unambiguous, and reject duplicate ids across extensions (consistent with the first-wins + warning namespace rule).
- **Confirm the `pub` re-exports earn their keep.** When Step 2 adds the manifest→registry wiring in `src/`, verify `ExtensionManifest`/`ExtensionView`/`ExtensionManifestError` are actually imported cross-crate; demote to `pub(crate)` only if the caller turns out to live inside the same crate.

## Status: Step 1 COMPLETE (2026-06-11)

All 3 tasks landed, each passing spec + code-quality review; final integration review: READY. Commits `c826f36`, `c7742d4`, `170ad21`. Evidence: `ozmux_extension_host` lib 61/61, `osc_webview` 7/7, `cargo clippy --workspace` clean, `cargo fmt --check` clean.
