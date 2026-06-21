# Switch ozmux to the bevy_cef `security` branch (PR #61)

Date: 2026-06-21
Status: Approved (design) — pending implementation

## Goal

Eliminate the security-relaxing Chromium switches ozmux inherits from
`bevy_cef` by moving its dependency from the `passthrough` branch to the
`security` branch ([not-elm/bevy_cef#61]). The PR makes CEF
secure-by-default: it removes the hardcoded `disable-web-security` (×2),
`allow-running-insecure-content`, `ignore-certificate-errors`, and
`ignore-ssl-errors` switches from `on_before_child_process_launch`, and the
malformed `enable-logging=stderr`.

ozmux itself never sets any risky switch — they come entirely from
`bevy_cef`'s hardcoded child-process launch path, which is exactly what
PR #61 removes. This is therefore a dependency + hardening change, not a
feature change.

[not-elm/bevy_cef#61]: https://github.com/not-elm/bevy_cef/pull/61

## Background: branch topology

ozmux currently depends on `bevy_cef` / `bevy_cef_core` at
`branch = "passthrough"`, pinned in `Cargo.lock` to `700c4a6`.

Verified content relationships across the three branches:

- `main` already contains every feature `passthrough` has —
  `WebviewTextureTarget` and `CefKeyboardFilter` were squash-merged into
  `main` via PR #59. `main` is in fact *ahead* of `passthrough`: it also
  carries PR #60's macOS ⌘C/X/V/A/Z clipboard shortcuts, which
  `passthrough` does not yet have.
- The `security` branch (PR #61) is based on `main`.
- Therefore the file-level delta `passthrough → security` is purely
  **additive**: `security` = passthrough's features + PR #60 clipboard
  shortcuts + the security hardening. Switching ozmux loses nothing; it
  gains clipboard support and the secure defaults.

Consequence: `passthrough` can effectively be retired. ozmux tracks the
`security` branch now (decision below); once #61 merges to `main`, a
one-line follow-up re-points ozmux to `main`.

## Decisions

1. **Target branch:** track `security` directly and immediately (not
   "merge to main first", not "merge security into passthrough").
2. **Secure default:** keep it — change nothing in ozmux's
   `CommandLineConfig`. Verify by running the app; fix forward if a real
   cross-origin need appears (do **not** re-enable `disable-web-security`).
3. **Sandbox:** leave `CefPlugin::sandbox` unset → `SandboxMode::PlatformDefault`
   ("no behavior change" per the PR). Do not opt into `Enabled` (the macOS
   render process is not yet linked against `cef_sandbox`; `Enabled` would
   only `warn!`).

## Changes

### Dependency switch (the mechanical change)

Flip `passthrough` → `security` in four places, then re-pin the lockfile:

| File | Line | Change |
| --- | --- | --- |
| `Cargo.toml` | 26 | `bevy_cef` git dep `branch = "passthrough"` → `"security"` |
| `Cargo.toml` | 92 | `[workspace.dependencies] bevy_cef_core` `branch` → `"security"` |
| `crates/ozma_webview/Cargo.toml` | 17 | `bevy_cef` `branch` → `"security"` |
| `justfile` | 11 | `bevy_cef_branch := "passthrough"` → `"security"` |

Then `cargo update -p bevy_cef -p bevy_cef_core` to repin `Cargo.lock` from
`700c4a6` to the `security` HEAD (currently `552dbd0`). This matches the
existing convention: human-readable `branch` in `Cargo.toml`, exact sha
pinned in `Cargo.lock`.

`justfile:11` (`bevy_cef_branch`) feeds only the **release** render-process
build (`setup-cef-release` / bundling). Keeping it consistent avoids a
version skew between the bundled render process and the linked CEF crate.

### Source changes

None in the base case. ozmux source changes materialize **only if**
verification (below) finds a genuine cross-origin dependency. In that case
the fix is to emit `Access-Control-*` headers from
`OzmuxDynScheme::handle` in `crates/webview_host/src/dyn_scheme.rs` (today
it returns `headers: Vec::new()`), mirroring how the upstream
`cef://localhost/` scheme already sends permissive CORS headers — **not**
re-enabling `disable-web-security`.

## Why the secure default should hold for ozmux

- The `window.ozma` back-channel rides CEF **process messages**
  (`cef.emit` → `Receive<OzmuxFrame>`), not HTTP/`fetch`. The Same-Origin
  Policy does not touch it.
- Webview assets are served **same-origin per handle**
  (`ozma-dyn://<handle>/…`, one origin per handle), and the scheme is
  registered `STANDARD | SECURE | CORS_ENABLED | FETCH_ENABLED |
  DISPLAY_ISOLATED` (`crates/webview_host/src/dyn_scheme.rs`).

Same-origin asset loading and process-message IPC do not require a relaxed
SOP. This is the same reasoning PR #61 applied to `cef://localhost/`; the
only thing the PR could not do was a runtime check, which §Verification
covers here.

## Verification

1. `cargo build` and `cargo test --workspace` — compiles and unit tests
   pass against the `security` branch.
2. Static diligence: grep `sdk/`, the webview preload/bridge, and any
   mounted page assets for cross-origin `fetch(` / absolute-URL loads;
   confirm nothing assumes a relaxed Same-Origin Policy.
3. Runtime (macOS GUI, `cargo run`): mount a webview and confirm
   (a) the page renders, (b) `ozma-dyn://` assets load, (c)
   `window.ozma.call` / `on` round-trips end-to-end.
4. Confirm the new one-time startup `warn!` for risky switches does **not**
   fire — positive proof that no risk-relaxing switch is active.

## Operational notes

- **Debug render process** comes from crates.io
  (`bevy_cef_debug_render_process`); the PR does not change the pinned CEF
  version, so dev (`cargo run --features debug`) only needs a rebuild.
- **Release render process** is built from the branch; `just
  setup-cef-release` must be re-run before bundling (covered by the
  `justfile` bump above).

## Risks & rollback

- **Primary risk:** a mounted page needs cross-origin access → caught in
  Verification, fixed via CORS headers (above).
- **Branch churn:** `security` is an in-flight PR branch; a force-push /
  rebase is absorbed by re-running `cargo update`. After #61 merges to
  `main`, re-point ozmux to `main` (post-merge `main ⊇ security`).
- **Rollback:** revert the four-line branch change plus `Cargo.lock` —
  fully reversible, with no ozmux source changes in the base case.
