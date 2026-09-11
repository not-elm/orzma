# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Architecture

orzma is a terminal that runs as a single native GUI application; a built-in multiplexer (panes/windows) is planned. It is a hybrid Rust + TypeScript codebase organized as one Cargo workspace and one pnpm workspace sharing the same tree. There is no daemon, no HTTP server, and no browser-side frontend — terminal emulation, GPU rendering, layout, input, and webview rendering all run in one Bevy ECS world.

### Rust workspace (`Cargo.toml`)

The workspace root package is the one and only binary; library crates live under `crates/`. Edition 2024, toolchain pinned to `1.95` (`rust-toolchain.toml`).

- `orzma` (workspace root, `src/main.rs`) — the single binary: a Bevy 0.19 app. `main()` builds one `App` and adds `DefaultPlugins` (configured with a `WindowPlugin` titled "orzma") plus `cef_plugin(orzma_registry.clone(), cef_profile.path())` (from `bevy_cef`), then the orzma plugins:
  - `SurfacePlugin`, `SessionPlugin`, `OrzmuxPlugin` (from `bevy_orzmux`), `TerminalRendererPlugin` (from `bevy_orzma_tty_renderer`), `ActionPlugin`, `OrzmaConfigsPlugin`, `FontBridgePlugin`, `OrzmaInputPlugin` (`input`'s root plugin, aggregating `ShortcutsPlugin`, `OptionAsAltPlugin`, `KeyboardInputPlugin`, `MouseInputPlugin`, `FocusSyncPlugin`, `ImePlugin`, and `HyperlinkInputPlugin`), `OrzmaUiPlugin` (`ui`'s root plugin, aggregating the UI root, the shell-surface subtree, the IME overlay, and the vi-mode indicator);
  - `OrzmaWebviewPlugin` (from `bevy_orzma_webview`), `WindowTitlePlugin`.
  - The in-process webview feature — CEF render wiring, the control-socket listener, the `window.orzma` back-channel, the APC and control-socket `mount` / `unmount` verbs, and webviews — is aggregated under `OrzmaWebviewPlugin` (from `crates/bevy_orzma_webview`).

  The root `Cargo.toml` depends on `bevy_orzma_webview` (path dep) and on `bevy_cef` (crates.io, `0.12`). A root `[features] debug` flag (forwarded through `bevy_orzma_webview/debug` to `bevy_cef/debug`) enables the CEF `remote-debugging-port` (a local Chromium DevTools / CDP endpoint on `127.0.0.1:9222`) for inspecting the embedded webview; it is off by default (`cargo run --features debug`).

- `crates/orzma_vt` (`orzma_vt`) — VT emulation: the `Vt` trait, `OrzmaVt`'s screen/grid/viewport model, CSI/OSC/APC dispatch, selection, vi-cursor state, and the palette/color types. No Bevy or PTY dependency; consumed by `orzma_tty`, `orzmux`, `bevy_orzmux`, and `bevy_orzma_tty_renderer`.
- `crates/orzma_tty` (`orzma_tty`) — PTY-backed terminal core: spawns the login shell under a PTY (or, for tests, a PTY-less fake master) and drives an injected `Vt` implementor behind a frame coalescer. Exposes `OrzmaTty`, `SpawnOptions`, and the key / mouse / paste input encoders; no Bevy dependency.
- `crates/orzmux` (`orzmux`) — the built-in multiplexer backend: a Bevy-free thread that owns every pane's PTY-backed `OrzmaTty<OrzmaVt>` and the cell-unit split tree (`layout`), waits on the command channel and each pane's PTY streams with one `crossbeam_channel::Select`, and emits `OrzmuxEvent`s (`Layout` / `Frame` / `Signal` / `PaneOpened` / `PaneClosed` / `SelectionText`) to the GUI. The GUI holds an `OrzmuxClient` and sends `OrzmuxCommand`s (`Resize`, `NewPane`, `KillPane`, `SelectPane*`, key / mouse / paste / scroll / selection input). Every type crossing the channel is plain data so the transport can later become a socket to a separate process.
- `crates/bevy_orzmux` (`bevy_orzmux`) — Bevy integration for `orzmux`: the `OrzmuxConnection` resource, the `OrzmuxPane(PaneId)` component each pane entity carries (with `TtyTitle` and, via the renderer, `TerminalGrid` as required components), `drain_orzmux_events` (turns backend events into `Tty*Signal` `EntityEvent`s and entity lifecycle), `apply_layout` (absolute pane nodes and separators from the latest `Layout`), and the inbound `RequestTty*` / `RequestActive*` / `RequestPaneAction` observers that send `OrzmuxCommand`s. Exposes `OrzmuxPlugin`.
- `crates/bevy_orzma_tty_renderer` (`bevy_orzma_tty_renderer`) — GPU terminal renderer plus the grid schema `bevy_orzmux` populates via `TtyFrameSignal`. `TerminalRendererPlugin` wires the grid, material, and glyph sub-plugins (`TerminalGridPlugin`, `TerminalMaterialPlugin`, `TerminalGlyphPlugin`) and hyperlink-hover state; `schema` holds the cell/grid types both crates render against.
- `crates/bevy_orzma_webview` (`bevy_orzma_webview`) — the in-process webview feature: depends on `orzma_vt`, `bevy_orzmux`, `bevy_orzma_tty_renderer`, `bevy_orzma_webview_host`, and `bevy_cef`. Aggregates CEF render wiring, the APC and control-socket `mount` / `unmount` handler, the `window.orzma` back-channel, the control-socket listener that mints Tier 1 dynamic webview handles, focus management, and the CPU paint bridge that copies CEF's `on_paint` frames into each webview's headless `WebviewTextureTarget` on Windows and Linux (`bevy_cef` writes that target only on macOS). Exposes `OrzmaWebviewPlugin` and `cef_plugin`.
- `crates/bevy_orzma_webview_host` (`bevy_orzma_webview_host`) — Tokio-free webview host integration for orzma: a per-handle `RuntimeRoot` runtime directory tree (the 0700 socket dir the control plane mints), and (behind the `cef` feature) serving dynamically-registered Tier 1 webview assets from disk/memory through a `bevy_cef` `orzma://` custom scheme via the `bevy_cef_core` path dep. The `cef` feature is off by default so the core builds/tests with std only. Exposes `WebviewAsset`, `WebviewAssetRegistry`, `custom_orzma_scheme`, and `RuntimeRoot`.
- `crates/orzma_configs` (`orzma_configs`) — config loader. Reads `~/.config/orzma/config.toml` (or `$ORZMA_CONFIG` / `$XDG_CONFIG_HOME` overrides) and resolves it against built-in defaults.
- **Platforms.** macOS is the primary platform. Windows 10 1809+ / 11 (x64) is
  supported for the `orzma` binary, the `ratatui_orzma` SDK, and the companion
  apps (`apps/orzmd`, `apps/orzbrowser`); the SDK reaches the control socket
  through `uds_windows` there and mounts webviews with the socket `mount` op,
  since ConPTY drops the APC verb. Linux is planned.

In-process webview rendering is provided by the external `bevy_cef` crate (crates.io `0.12`, CEF v149 pinned to `149.3.0+149.0.6` in the justfile). Both the renderer and the helper render process come from `bevy_cef` / `export-cef-dir`; see `just setup-cef`.

### TypeScript workspace (`pnpm-workspace.yaml`)

`packageManager` is `pnpm@10.30.2`. `catalogMode: strict` — shared versions for `@types/node`, `typescript`, `vitest` live under `pnpm-workspace.yaml`'s `catalog:`. Workspace packages are `sdk/*`:

- `sdk/orzma-web` (`@orzma/web`) — in-page TypeScript client for the `window.orzma` bridge (`orzma`, `isOrzmaAvailable`, `OrzmaApi`); tests via `vitest`.

### How the pieces connect at runtime

1. `orzma` boots a single Bevy `App`, starts the `orzmux` backend thread with `OrzmuxClient::spawn`, sends the window geometry (`OrzmuxCommand::Resize`) once font metrics exist, and requests the first pane (`PaneSpawnRequest { Root }`). The backend spawns the login shell under a PTY driving an `orzma_vt::OrzmaVt`, and streams `Layout` snapshots plus coalesced `Frame`s that `bevy_orzma_tty_renderer` draws on the GPU. Splits, directional selection, and kills are `OrzmuxCommand`s; the backend owns the active pane and the GUI mirrors it into `KeyboardFocused`.
2. A program registers webview content over the control socket (`OrzmaWebviewPlugin`, from `crates/bevy_orzma_webview`) to mint an opaque handle and its first placement instance, then writes an APC `Omount;n=<instance>` sequence to mount it as an in-process `bevy_cef` webview (assets served from disk/memory via `orzma://`, one origin per handle); where the PTY drops APC (ConPTY on Windows), the program sends the equivalent `mount` / `unmount` ops over the control socket instead. Additional placements of the same handle are minted with `new_instance` over the same socket. The page talks back to the registering program through `window.orzma.call/on` routed over the control socket.

### `src/` module map

`src/main.rs` plus: `action`, `cef_profile`, `configs`, `font`, `input`, `session`, `surface`, `system_set`, `ui`, `window_title`.

## Commands

### Rust

| Action                  | Command                                                                            |
| ----------------------- | ---------------------------------------------------------------------------------- |
| Build the workspace     | `cargo build` (or `just build`)                                                    |
| Run the app             | `cargo run` (or `just run`)                                                         |
| Run all tests           | `cargo test`                                                                        |
| Run one crate's tests   | `cargo test -p orzma_configs` (e.g. `cargo test -p orzma_configs <name>`)          |
| Lint + format (Rust)    | `cargo clippy --workspace --fix --allow-dirty --allow-staged && cargo fmt`         |
| Fix everything          | `just fix-lint` (runs clippy fix, rustfmt, and `pnpm lint:fix`)                     |
| Provision CEF (one-time) | `just setup-cef` (installs the CEF framework + render process; macOS and Windows) |

Logs go through `tracing-subscriber`; override the filter with `RUST_LOG`.

### TypeScript

| Action                  | Command                                                |
| ----------------------- | ------------------------------------------------------ |
| Install workspace deps  | `pnpm install`                                         |
| Run all vitest suites   | `pnpm -r test`                                         |
| Typecheck every package | `pnpm check-types`                                     |
| Lint (biome)            | `pnpm lint` / `pnpm lint:fix` / `pnpm lint:ci`         |

Biome (`biome.json`) scans `sdk/**` — it is the JS/TS lint+format tool for this repo.

## Other notable paths

- `.claude/rules/` — repo-wide Rust and TypeScript conventions (linked from the rules sections below).
- `docs/` — design notes and specs (tracked in git).

## Comment language

All in-code comments — line comments (`//`), doc comments (`///`, `//!`),
and block comments in any language under this repo — must be written in
English. This applies to Rust (`src/`, `crates/*`), TypeScript
(`sdk/*`, `extensions/*`), shell scripts, and config files. Use English
even when the conversation with the user is in another language.
Identifiers and string literals are not constrained by this rule; only
comments are.

## Rust Coding Rules

Rust style and conventions (no `mod.rs`, restricted comment taxonomy,
doc-comment policy, import discipline) are governed by
[`.claude/rules/rust.md`](.claude/rules/rust.md). Applies to the root
binary (`src/`) and all crates under `crates/`.

## TypeScript Coding Rules

TypeScript style and conventions (restricted comment taxonomy, JSDoc on
exports, export-visibility minimization, justified suppressions) are
governed by [`.claude/rules/typescript.md`](.claude/rules/typescript.md).
Applies to `sdk/*` and `extensions/*`.
