# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Architecture

orzma is a terminal that runs as a single native GUI application; a built-in multiplexer (panes/windows) is planned — see the local-multiplexer design. It is a hybrid Rust + TypeScript codebase organized as one Cargo workspace and one pnpm workspace sharing the same tree. There is no daemon, no HTTP server, and no browser-side frontend — terminal emulation, GPU rendering, layout, input, and webview rendering all run in one Bevy ECS world.

### Rust workspace (`Cargo.toml`)

The workspace root package is the one and only binary; library crates live under `crates/`. Edition 2024, toolchain pinned to `1.95` (`rust-toolchain.toml`).

- `orzma` (workspace root, `src/main.rs`) — the single binary: a Bevy 0.19 app. `main()` builds one `App` and adds `DefaultPlugins` (configured with a `WindowPlugin` titled "orzma") plus `cef_plugin(orzma_registry.clone(), cef_profile.path())` (from `bevy_cef`), then the orzma plugins:
  - `SurfacePlugin`, `DefaultSessionPlugin`, `TerminalHandlePlugin` (from `orzma_tty_engine`), `TerminalRendererPlugin` (from `orzma_tty_renderer`), `ActionPlugin`, `OrzmaConfigsPlugin`, `FontBridgePlugin`, `OrzmaBootstrapPlugin`, `OrzmaInputPlugin` (`input`'s root plugin, aggregating `ShortcutsPlugin`, `OptionAsAltPlugin`, `KeyboardInputPlugin`, `MouseInputPlugin`, and the default-mode input dispatcher), `OrzmaUiPlugin` (`ui`'s root plugin, aggregating the UI root plus the default-mode UI subtree);
  - `OrzmaWebviewPlugin` (from `orzma_webview`), `ViModePlugin`, `ViModeIndicatorPlugin`, `WindowTitlePlugin`;
  - the input plugins `FocusSyncPlugin`, `HyperlinkInputPlugin`, `ImePlugin`, and `ImeOverlayPlugin`.
  - The in-process webview feature — CEF render wiring, the control-socket listener, the `window.orzma` back-channel, OSC 5379 `mount` / `unmount`, and webviews — is aggregated under `OrzmaWebviewPlugin` (from `crates/orzma_webview`).

  The root `Cargo.toml` depends on `orzma_webview` (path dep) and on `bevy_cef` (crates.io, `0.12`). A root `[features] debug` flag (forwarded through `orzma_webview/debug` to `bevy_cef/debug`) enables the CEF `remote-debugging-port` (a local Chromium DevTools / CDP endpoint on `127.0.0.1:9222`) for inspecting the embedded webview; it is off by default (`cargo run --features debug`).

- `crates/orzma_tty_engine` (`orzma_tty_engine`) — Bevy-native terminal: PTY ownership and `alacritty_terminal` VT emulation, emitting coalesced `FrameSnapshot` / `FrameDelta` against the `orzma_tty_renderer` schema. Exposes `TerminalHandlePlugin`.
- `crates/orzma_tty_renderer` (`orzma_tty_renderer`) — GPU terminal renderer plus the grid schema shared with `orzma_tty_engine`. `TerminalRendererPlugin` wires the grid, material, and glyph sub-plugins (`TerminalGridPlugin`, `TerminalMaterialPlugin`, `TerminalGlyphPlugin`) and hyperlink-hover state; `schema` holds the cell/grid types both crates render against.
- `crates/orzma_webview` (`orzma_webview`) — the in-process webview feature: depends on `orzma_tty_engine`, `orzma_webview_host`, and (behind the `cef` feature) `bevy_cef`. Aggregates CEF render wiring, the OSC 5379 `mount` / `unmount` handler, the `window.orzma` back-channel, the control-socket listener that mints Tier 1 dynamic webview handles, and focus management. Exposes `OrzmaWebviewPlugin` and `cef_plugin`.
- `crates/webview_host` (`orzma_webview_host`) — Tokio-free webview host integration for orzma: a per-handle `RuntimeRoot` runtime directory tree (the 0700 socket dir the control plane mints), and (behind the `cef` feature) serving dynamically-registered Tier 1 webview assets from disk/memory through a `bevy_cef` `orzma://` custom scheme via the `bevy_cef_core` path dep. The `cef` feature is off by default so the core builds/tests with std only. Exposes `WebviewAsset`, `WebviewAssetRegistry`, `custom_orzma_scheme`, and `RuntimeRoot`.
- `crates/orzma_tmux` (`orzma_tmux`) — a tmux `-CC` control-mode client: owns the connection, drains its transport events into the Bevy world, tracks connection lifecycle, and projects tmux session/window/pane state as ECS entities (`TmuxSession` / `TmuxWindow` / `TmuxPane`). Exposes `TmuxSessionPlugin`. (Built on `crates/tmux_control` and `crates/tmux_control_parser`, the sans-io tmux control-mode client and parser.) Not referenced by the `orzma` binary — kept as workspace members, building and testing in CI, pending the built-in multiplexer that replaces this integration.
- `crates/orzma_configs` (`orzma_configs`) — config loader. Reads `~/.config/orzma/config.toml` (or `$ORZMA_CONFIG` / `$XDG_CONFIG_HOME` overrides) and resolves it against built-in defaults.

In-process webview rendering is provided by the external `bevy_cef` crate (crates.io `0.12`, CEF v149 pinned to `149.3.0+149.0.6` in the justfile). Both the renderer and the helper render process come from `bevy_cef` / `export-cef-dir`; see `just setup-cef`.

### TypeScript workspace (`pnpm-workspace.yaml`)

`packageManager` is `pnpm@10.30.2`. `catalogMode: strict` — shared versions for `@types/node`, `typescript`, `vitest` live under `pnpm-workspace.yaml`'s `catalog:`. Workspace packages are `sdk/*`:

- `sdk/orzma-web` (`@orzma/web`) — in-page TypeScript client for the `window.orzma` bridge (`orzma`, `isOrzmaAvailable`, `OrzmaApi`); tests via `vitest`.

### How the pieces connect at runtime

1. `orzma` boots a single Bevy `App` and spawns one PTY-backed `OrzmaTerminal` entity: `orzma_tty_engine` spawns the shell PTY directly (no `tmux -CC` attach) and drives `alacritty_terminal` VT emulation, emitting frame snapshots/deltas that `orzma_tty_renderer` draws on the GPU. Layout, input, vi mode, IME, and shortcuts are all plugins in the same world.
2. A program registers webview content over the control socket (`OrzmaWebviewPlugin`, from `crates/orzma_webview`) to mint an opaque handle, then writes an OSC 5379 `mount;<handle>` sequence to mount it as an in-process `bevy_cef` webview (assets served from disk/memory via `orzma://`, one origin per handle). The page talks back to the registering program through `window.orzma.call/on` routed over the control socket.

### `src/` module map

`src/main.rs` plus: `action`, `bootstrap`, `cef_profile`, `configs`, `font`, `input`, `session`, `surface`, `system_set`, `theme`, `ui`, `window_title`.

## Commands

### Rust

| Action                  | Command                                                                            |
| ----------------------- | ---------------------------------------------------------------------------------- |
| Build the workspace     | `cargo build` (or `just build`)                                                    |
| Run the app             | `cargo run` (or `just run`)                                                         |
| Run all tests           | `cargo test`                                                                        |
| Run one crate's tests   | `cargo test -p orzma_tmux` (e.g. `cargo test -p orzma_tmux <name>`)                |
| Lint + format (Rust)    | `cargo clippy --workspace --fix --allow-dirty --allow-staged && cargo fmt`         |
| Fix everything          | `just fix-lint` (runs clippy fix, rustfmt, and `pnpm lint:fix`)                     |
| Provision CEF (one-time) | `just setup-cef` (installs the CEF framework + debug render process; macOS)        |

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
