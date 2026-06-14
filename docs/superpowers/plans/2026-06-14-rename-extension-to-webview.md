# Rename `extension_*` → `webview_*` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rename all live `extension`-named code to role-specific `webview_*` names and delete the dead remnants of the removed Extension feature.

**Architecture:** Pure rename/refactor in an existing Cargo + pnpm workspace. The crate `ozmux_extension_host` becomes `ozmux_webview_host`; the binary module `extension_render` becomes `webview_render`; dead config/shortcut/preload remnants are deleted. Public API symbol names (`DynAsset`, `DynAssetRegistry`, `custom_dyn_scheme`, `RuntimeRoot`) are kept. "File extension" semantics in MIME code are NOT touched.

**Tech Stack:** Rust (edition 2024, toolchain 1.95), Bevy 0.18, `bevy_cef`; pnpm/TypeScript SDK (unaffected). Verification via `cargo build`, `cargo test`, `cargo clippy --workspace`, `cargo fmt`, `pnpm -r test`.

**Note on TDD:** This is a mechanical rename. The existing test suite is the safety net — each task ends by running the build/tests to prove green, then commits. No new "failing test first" cycle applies.

Spec: `docs/superpowers/specs/2026-06-14-rename-extension-to-webview-design.md`

---

### Task 1: Delete dead remnants in the `ozmux_configs` crate

`extensions_dir()` / `EXTENSIONS_REL_PATH` and `NewExtensionSurface` have no live callers (only self-tests / the definition). Delete them.

**Files:**
- Modify: `crates/configs/src/path.rs`
- Modify: `crates/configs/src/shortcuts.rs`

- [ ] **Step 1: Remove the `EXTENSIONS_REL_PATH` const**

In `crates/configs/src/path.rs`, delete line 12:

```rust
const EXTENSIONS_REL_PATH: &str = "ozmux/extensions";
```

- [ ] **Step 2: Remove the `extensions_dir` function**

In `crates/configs/src/path.rs`, delete the entire item (the doc comment + fn, currently lines 61–75):

```rust
/// Returns the directory that ozmux scans for user-installed extensions.
///
/// Precedence: `$XDG_CONFIG_HOME/ozmux/extensions` →
/// `<home_dir>/.config/ozmux/extensions`. Returns `HomeDirNotFound` only
/// when both lookups fail. `$OZMUX_CONFIG` is intentionally not consulted
/// because it points to a config file, not a directory.
pub fn extensions_dir(env: &dyn Env) -> OzmuxConfigsResult<PathBuf> {
    if let Some(xdg) = env.var(ENV_XDG_CONFIG_HOME) {
        return Ok(PathBuf::from(xdg).join(EXTENSIONS_REL_PATH));
    }
    if let Some(home) = env.home_dir() {
        return Ok(home.join(HOME_CONFIG_DIR).join(EXTENSIONS_REL_PATH));
    }
    Err(OzmuxConfigsError::HomeDirNotFound)
}
```

- [ ] **Step 3: Remove the four `extensions_dir_*` tests**

In `crates/configs/src/path.rs`, delete these four test functions in the `#[cfg(test)] mod tests` block (currently lines 197–244):

```rust
    #[test]
    fn extensions_dir_uses_xdg_when_set() {
        let env = FakeEnv {
            vars: HashMap::from([("XDG_CONFIG_HOME".into(), "/tmp/foo".into())]),
            home: Some(PathBuf::from("/home/u")),
        };
        assert_eq!(
            extensions_dir(&env).unwrap(),
            PathBuf::from("/tmp/foo/ozmux/extensions")
        );
    }

    #[test]
    fn extensions_dir_falls_back_to_home_config() {
        let env = FakeEnv {
            vars: HashMap::new(),
            home: Some(PathBuf::from("/home/u")),
        };
        assert_eq!(
            extensions_dir(&env).unwrap(),
            PathBuf::from("/home/u/.config/ozmux/extensions")
        );
    }

    #[test]
    fn extensions_dir_ignores_ozmux_config_var() {
        let env = FakeEnv {
            vars: HashMap::from([("OZMUX_CONFIG".into(), "/tmp/x.toml".into())]),
            home: Some(PathBuf::from("/home/u")),
        };
        assert_eq!(
            extensions_dir(&env).unwrap(),
            PathBuf::from("/home/u/.config/ozmux/extensions"),
            "OZMUX_CONFIG points to a file, not a directory, and must not affect extensions_dir"
        );
    }

    #[test]
    fn extensions_dir_errors_when_no_xdg_and_no_home() {
        let env = FakeEnv {
            vars: HashMap::new(),
            home: None,
        };
        assert!(matches!(
            extensions_dir(&env).unwrap_err(),
            OzmuxConfigsError::HomeDirNotFound
        ));
    }
```

- [ ] **Step 4: Remove the `NewExtensionSurface` shortcut variant**

In `crates/configs/src/shortcuts.rs`, delete the variant + its doc (currently lines 600–601):

```rust
    /// Add a new extension surface to the active pane.
    NewExtensionSurface,
```

- [ ] **Step 5: Verify the configs crate builds and tests pass**

Run: `cargo test -p ozmux_configs`
Expected: PASS, no reference to `extensions_dir` / `NewExtensionSurface` errors.

Run: `cargo clippy -p ozmux_configs`
Expected: no warnings about the removed items.

- [ ] **Step 6: Commit**

```bash
git add crates/configs/src/path.rs crates/configs/src/shortcuts.rs
git commit -m "refactor(configs): drop dead extension remnants (extensions_dir, NewExtensionSurface)"
```

---

### Task 2: Rename the crate `extension_host` → `webview_host`

Atomic rename: the directory, package name, root dependency, and every `use ozmux_extension_host::…` must change together to compile.

**Files:**
- Move: `crates/extension_host/` → `crates/webview_host/`
- Modify: `crates/webview_host/Cargo.toml` (package name)
- Modify: `Cargo.toml` (root dependency line)
- Modify: `src/main.rs`, `src/control_plane.rs`, `src/extension_render.rs` (`use` paths)

- [ ] **Step 1: Move the crate directory with git**

Run:
```bash
git mv crates/extension_host crates/webview_host
```

- [ ] **Step 2: Rename the package in the crate manifest**

In `crates/webview_host/Cargo.toml`, change line 2:

```toml
name = "ozmux_webview_host"
```

(was `name = "ozmux_extension_host"`)

- [ ] **Step 3: Update the root dependency**

In `Cargo.toml`, replace line 36:

```toml
ozmux_webview_host = { path = "crates/webview_host", features = ["cef"] }
```

(was `ozmux_extension_host = { path = "crates/extension_host", features = ["cef"] }`)

- [ ] **Step 4: Update `use` paths in `src/main.rs`**

In `src/main.rs`, change line 36:

```rust
use ozmux_webview_host::DynAssetRegistry;
```

- [ ] **Step 5: Update `use` paths in `src/control_plane.rs`**

In `src/control_plane.rs`, change lines 13–14:

```rust
use ozmux_webview_host::DynAssetRegistry;
use ozmux_webview_host::host::RuntimeRoot;
```

And in the test at line 680:

```rust
        use ozmux_webview_host::DynAsset;
```

- [ ] **Step 6: Update `use` paths in `src/extension_render.rs`**

In `src/extension_render.rs`, change lines 12–13:

```rust
use ozmux_webview_host::DynAssetRegistry;
use ozmux_webview_host::dyn_scheme::custom_dyn_scheme;
```

- [ ] **Step 7: Verify the whole workspace builds and tests pass**

Run: `cargo build`
Expected: PASS (no `ozmux_extension_host` unresolved-crate errors).

Run: `cargo test`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "refactor: rename crate ozmux_extension_host -> ozmux_webview_host"
```

---

### Task 3: Rename the binary module `extension_render` → `webview_render`

Move the module file + its subdirectory, rename the plugin type, and update all references.

**Files:**
- Move: `src/extension_render.rs` → `src/webview_render.rs`
- Move: `src/extension_render/` → `src/webview_render/` (contains `preload.rs`, `ozmux_bridge.js`)
- Modify: `src/webview_render.rs` (plugin type name)
- Modify: `src/main.rs` (`mod`, `use`, plugin registration)
- Modify: `src/inline_webview.rs` (`use` path)

- [ ] **Step 1: Move the module file and directory with git**

Run:
```bash
git mv src/extension_render.rs src/webview_render.rs
git mv src/extension_render src/webview_render
```

- [ ] **Step 2: Rename the plugin type in `src/webview_render.rs`**

In `src/webview_render.rs`, change the struct + impl (currently lines 59 and 61):

```rust
pub struct OzmuxWebviewRenderPlugin;

impl Plugin for OzmuxWebviewRenderPlugin {
```

(was `OzmuxExtensionRenderPlugin`)

- [ ] **Step 3: Update `src/main.rs` module declaration**

In `src/main.rs`, change line 8:

```rust
mod webview_render;
```

- [ ] **Step 4: Update `src/main.rs` use + plugin registration**

In `src/main.rs`, change line 21:

```rust
use crate::webview_render::{OzmuxWebviewRenderPlugin, cef_plugin};
```

And the plugin registration at line 68:

```rust
            OzmuxWebviewRenderPlugin,
```

- [ ] **Step 5: Update `src/inline_webview.rs` use path**

In `src/inline_webview.rs`, change line 9:

```rust
use crate::webview_render::preload::build_dynamic_preload;
```

- [ ] **Step 6: Verify build + tests**

Run: `cargo build`
Expected: PASS.

Run: `cargo test`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "refactor: rename module extension_render -> webview_render (OzmuxWebviewRenderPlugin)"
```

---

### Task 4: Delete the dead `extension_name` param + `extensionName` JS field

`context_preload_js_role`'s `extension_name` is always `""`, and no JS/TS consumer reads `__ozmuxContext.extensionName`. Remove both.

**Files:**
- Modify: `src/webview_render/preload.rs`

- [ ] **Step 1: Drop the `extension_name` arg from `build_dynamic_preload`'s call**

In `src/webview_render/preload.rs`, change the body of `build_dynamic_preload`:

```rust
    let ctx_js = context_preload_js_role(workspace, pane, surface, "dynamic");
```

(was `…, "dynamic", "")`)

- [ ] **Step 2: Update `context_preload_js_role` signature, doc, and format string**

In `src/webview_render/preload.rs`, replace the doc comment + function down to the `format!` block with:

```rust
/// Builds the per-webview context PreloadScript assigning `window.__ozmuxContext`
/// with the given `role`.
///
/// NOTE: the JS keys "sessionId"/"windowId" keep their legacy names on purpose — a
/// browser-side wire contract the SDK surface client reads; renaming them breaks the SDK.
fn context_preload_js_role(
    workspace: Entity,
    pane: Entity,
    surface: Entity,
    role: &str,
) -> String {
    let workspace_id = workspace.to_bits().to_string();
    format!(
        "window.__ozmuxContext={{sessionId:{s:?},windowId:{s:?},paneId:{p:?},surfaceId:{a:?},role:{r:?}}};",
        s = workspace_id,
        p = pane.to_bits().to_string(),
        a = surface.to_bits().to_string(),
        r = role,
    )
}
```

(removed the `extension_name: &str` param, the `,extensionName:{n:?}` field, and the `n = extension_name,` argument)

- [ ] **Step 3: Update the test call site**

In `src/webview_render/preload.rs`, in the test `context_preload_js_role_assigns_window_context_with_workspace_bits_as_window_id`, change the call (currently passes `"dynamic", ""`):

```rust
        let js = context_preload_js_role(workspace, pane, surface, "dynamic");
```

- [ ] **Step 4: Verify build + the preload tests pass**

Run: `cargo test --bin ozmux-gui webview_render::preload`
Expected: PASS (both `context_preload_js_role_*` and `dynamic_preload_injects_context_and_ozmux_bridge`).

If the bin name differs, fall back to: `cargo test preload`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/webview_render/preload.rs
git commit -m "refactor(webview_render): drop dead extension_name param + extensionName JS field"
```

---

### Task 5: Sweep `extension` out of comments (keep file-extension semantics)

Replace removed-feature comment wording. Do NOT touch genuine "file extension" / MIME wording or the `material.rs` "wire extension".

**Files:**
- Modify: `Cargo.toml`
- Modify: `crates/webview_host/src/host.rs`
- Modify: `crates/webview_host/src/asset.rs`
- Modify: `src/webview_render.rs`
- Modify: `src/input.rs`
- Modify: `src/clipboard.rs`
- Modify: `src/input/mouse_buttons.rs`

- [ ] **Step 1: Root `Cargo.toml` feature comment**

In `Cargo.toml`, change line 17 from:

```toml
# extension webview. Off by default so the endpoint is not exposed in normal
```

to:

```toml
# embedded webview. Off by default so the endpoint is not exposed in normal
```

- [ ] **Step 2: `crates/webview_host/src/host.rs` comments + the `/tmp/ozmux-ext` fallback path**

The `/tmp/ozmux-ext` path is a fallback socket dir minted fresh per run and removed on `Drop` — no persistent-state compat concern — so rename it too.

Apply these exact replacements:

- Doc comment on `resolve_in`: change `/tmp/ozmux-ext` → `/tmp/ozmux-webview` (in `/// `/tmp/ozmux-ext` when the socket path would overflow…`).
- The path literal: `let fallback = Path::new("/tmp/ozmux-ext");` → `let fallback = Path::new("/tmp/ozmux-webview");`
- `// NOTE: measure the LONGEST socket filename a command extension uses` → `// NOTE: measure the LONGEST socket filename a webview uses`
- `    /// The directory holding extension sockets.` → `    /// The directory holding webview sockets.`
- `"the intermediate <pid> dir must be 0700 so extension names do not leak"` → `"the intermediate <pid> dir must be 0700 so webview names do not leak"`
- `"same-PID extensions must not share a root"` → `"same-PID webviews must not share a root"`
- `"dropping one extension must not remove another's sockets"` → `"dropping one webview must not remove another's sockets"`

Do NOT change the `// NOTE: … like the legacy /tmp/ozmux` comment — that `/tmp/ozmux` names a different historical path, not the fallback being renamed.

- [ ] **Step 3: `crates/webview_host/src/asset.rs` comments (keep file-extension wording)**

Apply these exact replacements:

- Line 33: `/// page from reading files outside its extension directory.` → `/// page from reading files outside its webview directory.`
- Line 96: in the `// TODO:` line, replace `the extension dir` → `the webview dir` and `extension-dir contents` → `webview-dir contents`. Result:
  ```rust
  // TODO: lexical check only — a symlink inside the webview dir is still followed by std::fs::read; add a canonicalize + prefix check if webview-dir contents ever become untrusted (Phase 1 trusts them).
  ```
- Line 104: change ONLY `extension ships` → `webview ships`. The doc spans lines 103–104; the result must read:
  ```rust
  /// Maps a file extension to a bare MIME type for the asset set a Phase 1
  /// webview ships. Unknown extensions fall back to `application/octet-stream`.
  ```
  Do NOT change `Maps a file extension`, `Unknown extensions`, the `.extension()` call, or the `mime_for_common_extensions` test name.

- [ ] **Step 4: `src/webview_render.rs` comments**

Apply these exact replacements:

- Line 181: `/// observers fire for every `bevy_cef` webview, not only extension hosts.` → `/// observers fire for every `bevy_cef` webview, not only ozmux webviews.`
- Lines 216–217 (inside the test): `so bevy_cef blurs the extension webview (releasing its DOM text area` → `so bevy_cef blurs the webview (releasing its DOM text area`, and `and stopping keyboard from routing to it). When the extension pane is` → `and stopping keyboard from routing to it). When the webview pane is`
- Line 259: `// WebviewSource; the extension surface carries one.` → `// WebviewSource; the webview surface carries one.`
- Line 280: `"active extension pane must focus its webview"` → `"active webview pane must focus its webview"`

- [ ] **Step 5: `src/input.rs` comment**

In `src/input.rs`, line 518: `/// `TerminalHandle` (e.g. an extension surface) — the `ozma_tty_engine`` → `/// `TerminalHandle` (e.g. a webview surface) — the `ozma_tty_engine``

- [ ] **Step 6: `src/clipboard.rs` comment**

In `src/clipboard.rs`, line 203: `/// selection or a missing `TerminalHandle` (e.g. an extension surface).` → `/// selection or a missing `TerminalHandle` (e.g. a webview surface).`

- [ ] **Step 7: `src/input/mouse_buttons.rs` comments**

Apply these exact replacements:

- Line 494: `// Click-to-focus runs for EVERY pane kind (terminal, extension)` → `// Click-to-focus runs for EVERY pane kind (terminal, webview)`
- Line 496: `// `continue`s past panes with no `TerminalHandle` (extension` → `// `continue`s past panes with no `TerminalHandle` (webview`
- Line 1686: `// Regression: clicking an extension pane — a Surface entity` → `// Regression: clicking a webview pane — a Surface entity`

- [ ] **Step 8: Verify build, tests, fmt, clippy**

Run: `cargo build`
Expected: PASS.

Run: `cargo test`
Expected: PASS.

Run: `cargo clippy --workspace`
Expected: no new warnings.

Run: `cargo fmt`
Expected: no changes (or only whitespace it auto-fixes).

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "refactor: sweep extension wording out of comments (keep file-extension MIME)"
```

---

### Task 6: Update `CLAUDE.md`

Bring the live project doc in line with the new names. Do NOT touch historical files under `docs/superpowers/plans|specs/`.

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: Plugin name in the boot list (line 14)**

In `CLAUDE.md`, replace `OzmuxExtensionRenderPlugin` with `OzmuxWebviewRenderPlugin`.

- [ ] **Step 2: Root dependency sentence (line 18)**

In `CLAUDE.md`, replace `on `ozmux_extension_host` with the `cef` feature enabled` with `on `ozmux_webview_host` with the `cef` feature enabled`.

- [ ] **Step 3: Crate description (line 22)**

In `CLAUDE.md`, change the bullet that starts ``- `crates/extension_host` (`ozmux_extension_host`) —`` to ``- `crates/webview_host` (`ozmux_webview_host`) —`` (only the path and package name; the prose body is already webview-centric and stays).

- [ ] **Step 4: Module map (line 41)**

In `CLAUDE.md`, in the `src/` module map line, replace `extension_render` with `webview_render`.

- [ ] **Step 5: Verify no stale references remain in CLAUDE.md**

Run: `grep -n "extension_host\|extension_render\|OzmuxExtensionRender\|ozmux_extension_host" CLAUDE.md`
Expected: no output.

- [ ] **Step 6: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: update CLAUDE.md for webview_host / webview_render rename"
```

---

### Task 7: Final cross-cutting verification

Confirm the whole workspace is green and that the only remaining `extension` mentions are genuine file-extension semantics.

**Files:** none (verification only)

- [ ] **Step 1: Full Rust build + test + lint**

Run: `cargo build`
Expected: PASS.

Run: `cargo test`
Expected: PASS.

Run: `cargo clippy --workspace`
Expected: no warnings.

Run: `cargo fmt --check`
Expected: no diff.

- [ ] **Step 2: SDK tests (sanity — should be unaffected)**

Run: `pnpm -r test`
Expected: PASS.

- [ ] **Step 3: Audit remaining `extension` mentions**

Run: `grep -rin "extension" --include="*.rs" --include="*.toml" . | grep -v target`
Expected: ONLY these legitimate file-extension/wire references remain:
- `crates/webview_host/src/asset.rs` — `.extension()`, "Maps a file extension", "Unknown extensions", `mime_for_common_extensions`
- `crates/ozma_tty_renderer/src/material.rs` — "wire extension"

Any other hit is a missed rename — fix it and re-run the relevant task's verification before continuing.

- [ ] **Step 4: Confirm no leftover old paths**

Run: `ls crates/extension_host src/extension_render.rs src/extension_render 2>&1`
Expected: "No such file or directory" for all three.
