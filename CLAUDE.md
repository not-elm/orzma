# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Architecture

ozmux is a terminal multiplexer that runs as a single native GUI application. It is a hybrid Rust + TypeScript codebase organized as one Cargo workspace and one pnpm workspace sharing the same tree. There is no daemon, no HTTP server, and no browser-side frontend — terminal emulation, GPU rendering, layout, input, and webview rendering all run in one Bevy ECS world.

### Rust workspace (`Cargo.toml`)

The workspace root package is the one and only binary; library crates live under `crates/`. Edition 2024, toolchain pinned to `1.95` (`rust-toolchain.toml`).

- `ozmux-gui` (workspace root, `src/main.rs`) — the single binary: a Bevy 0.18 app. `main()` builds one `App` and adds `DefaultPlugins` (configured with a `WindowPlugin` titled "ozmux") plus `cef_plugin(&asset_endpoint)` (from `bevy_cef`), then the ozmux plugins:
  - `TerminalHandlePlugin` (from `ozma_tty_engine`), `TerminalRendererPlugin` (from `ozma_tty_renderer`), `TmuxSessionPlugin` (from `ozmux_tmux`, the sole multiplexer), `OzmuxTmuxPickerPlugin`, `OzmuxConfigsPlugin`, `FontBridgePlugin`, `OzmuxBootstrapPlugin`, `OzmuxShortcutPlugin`, `OzmuxUiPlugin`, `OzmuxWebviewPlugin`, `CopyModePlugin`, `CopyModeIndicatorPlugin`, and the tmux UI/IO plugins `CopyPromptPlugin`, `ConfirmPromptPlugin`, `TmuxDialogPlugin`, `OzmuxTmuxRenderPlugin`, `OzmuxTmuxInputPlugin`, `OzmuxTmuxWindowBarPlugin`, `OzmuxTmuxPaneFocusPlugin`, `OzmuxTmuxCopyModePlugin`, `OzmuxTmuxMousePlugin`, `OzmuxTmuxDividerHandlePlugin`;
  - the input plugins `HyperlinkInputPlugin`, `ImePlugin`, and `ImeOverlayPlugin`;
  - and `OzmuxControlPlanePlugin` (the control-socket listener that mints Tier 1 dynamic webview handles). The in-process webview feature — CEF render wiring, the `window.ozma` back-channel, OSC 5379 `mount-inline` / `unmount-inline`, and inline webviews — is aggregated under `OzmuxWebviewPlugin` (above).

  The root `Cargo.toml` depends on `bevy_cef` (path dep, `features = ["debug"]`) and on `ozmux_webview_host` with the `cef` feature enabled. A root `[features] debug` flag enables the CEF `remote-debugging-port` (a local Chromium DevTools / CDP endpoint on `127.0.0.1:9222`) for inspecting the embedded webview; it is off by default (`cargo run --features debug`).

- `crates/ozma_tty_engine` (`ozma_tty_engine`) — Bevy-native terminal: PTY ownership and `alacritty_terminal` VT emulation, emitting coalesced `FrameSnapshot` / `FrameDelta` against the `ozma_tty_renderer` schema. Exposes `TerminalHandlePlugin`.
- `crates/ozma_tty_renderer` (`ozma_tty_renderer`) — GPU terminal renderer plus the grid schema shared with `ozma_tty_engine`. `TerminalRendererPlugin` wires the grid, material, and glyph sub-plugins (`TerminalGridPlugin`, `TerminalMaterialPlugin`, `TerminalGlyphPlugin`) and hyperlink-hover state; `schema` holds the cell/grid types both crates render against.
- `crates/webview_host` (`ozmux_webview_host`) — Tokio-free webview host integration for ozmux: a per-handle `RuntimeRoot` runtime directory tree (the 0700 socket dir the control plane mints), and (behind the `cef` feature) serving dynamically-registered Tier 1 webview assets from disk/memory through a `bevy_cef` `ozma-dyn://` custom scheme via the `bevy_cef_core` path dep. The `cef` feature is off by default so the core builds/tests with std only. Exposes `WebviewAsset`, `WebviewAssetRegistry`, `custom_dyn_scheme`, and `RuntimeRoot`.
- `crates/tmux_session` (`ozmux_tmux`) — the sole multiplexer: owns a `tmux -CC` control-mode connection, drains its transport events into the Bevy world, tracks connection lifecycle, and projects tmux session/window/pane state as ECS entities (`TmuxSession` / `TmuxWindow` / `TmuxPane`). Rendering lives in `src/` (the tmux render/input/UI plugins), not here. Exposes `TmuxSessionPlugin`. (Built on `crates/tmux_control` and `crates/tmux_control_parser`, the sans-io tmux control-mode client and parser.)
- `crates/ozmux_configs` (`ozmux_configs`) — config loader. Reads `~/.config/ozmux/config.toml` (or `$OZMUX_CONFIG` / `$XDG_CONFIG_HOME` overrides) and resolves it against built-in defaults.

In-process webview rendering is provided by the external `bevy_cef` crate (a path dependency on CEF v145, pinned to `145.6.1+145.0.28` in the Makefile). Both the renderer and the helper render process come from `bevy_cef` / `export-cef-dir`; see `make setup-cef`.

### TypeScript workspace (`pnpm-workspace.yaml`)

`packageManager` is `pnpm@10.30.2`. `catalogMode: strict` — shared versions for `@types/node`, `typescript`, `vitest` live under `pnpm-workspace.yaml`'s `catalog:`. Workspace packages are `sdk/*`:

- `sdk/ozma-web` (`@ozma/web`) — in-page TypeScript client for the `window.ozma` bridge (`ozma`, `isOzmaAvailable`, `OzmaApi`); tests via `vitest`.

### How the pieces connect at runtime

1. `ozmux-gui` boots a single Bevy `App`. `TmuxSessionPlugin` (`ozmux_tmux`) attaches to (or creates) a `tmux -CC` session and projects its session/window/pane state as ECS entities; `ozma_tty_engine` runs the PTY/VT emulation behind it, emitting frame snapshots/deltas that `ozma_tty_renderer` (and the tmux render plugins in `src/`) draw on the GPU. Layout, input, copy-mode, IME, and shortcuts are all plugins in the same world.
2. A program registers webview content over the control socket (Tier 1, `OzmuxControlPlanePlugin`) to mint an opaque handle, then writes an OSC 5379 `mount-inline;<handle>` sequence to mount it as an in-process `bevy_cef` inline webview (assets served from disk/memory via `ozma-dyn://`, one origin per handle). The page talks back to the registering program through `window.ozma.call/on` routed over the control socket.

### `src/` module map

`src/main.rs` plus: `bootstrap`, `clipboard`, `configs`, `control_plane`, `font`, `input`, `system_set`, `theme`, `tmux_copy_mode`, `tmux_input`, `tmux_mouse`, `tmux_pane_hit`, `tmux_picker`, `tmux_render`, `ui`, `webview`.

## Commands

### Rust

| Action                  | Command                                                                            |
| ----------------------- | ---------------------------------------------------------------------------------- |
| Build the workspace     | `cargo build` (or `make build`)                                                    |
| Run the app             | `cargo run` (or `make run`)                                                         |
| Run all tests           | `cargo test`                                                                        |
| Run one crate's tests   | `cargo test -p ozmux_tmux` (e.g. `cargo test -p ozmux_tmux <name>`)                |
| Lint + format (Rust)    | `cargo clippy --workspace --fix --allow-dirty --allow-staged && cargo fmt`         |
| Fix everything          | `make fix-lint` (runs clippy fix, rustfmt, and `pnpm lint:fix`)                     |
| Provision CEF (one-time) | `make setup-cef` (installs the CEF framework + debug render process; macOS)        |

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
