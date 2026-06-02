# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Architecture

ozmux is a terminal multiplexer that runs as a single native GUI application. It is a hybrid Rust + TypeScript codebase organized as one Cargo workspace and one pnpm workspace sharing the same tree. There is no daemon, no HTTP server, and no browser-side frontend — terminal emulation, GPU rendering, layout, input, and webview rendering all run in one Bevy ECS world.

### Rust workspace (`Cargo.toml`)

The workspace root package is the one and only binary; library crates live under `crates/`. Edition 2024, toolchain pinned to `1.95` (`rust-toolchain.toml`).

- `ozmux-gui` (workspace root, `src/main.rs`) — the single binary: a Bevy 0.18 app. `main()` builds one `App` and adds `DefaultPlugins` (configured with a `WindowPlugin` titled "ozmux") plus `cef_plugin(&asset_endpoint)` (from `bevy_cef`), then the ozmux plugins:
  - `TerminalHandlePlugin` (from `bevy_terminal`), `TerminalRendererPlugin` (from `bevy_terminal_renderer`), `MultiplexerPlugin` (from `ozmux_multiplexer`), `OzmuxConfigsPlugin`, `FontBridgePlugin`, `OzmuxLayoutLogPlugin`, `OzmuxBootstrapPlugin`, `OzmuxShortcutPlugin`, `OzmuxUiPlugin`, `OzmuxExtensionRenderPlugin`, `CopyModePlugin`, `CopyModeIndicatorPlugin`;
  - the input plugins `MouseWheelInputPlugin`, `MouseButtonsInputPlugin`, `HyperlinkInputPlugin`, `ImePlugin`, `ImeOverlayPlugin`, and `OzmuxShortcutActionPlugin`;
  - `ExtensionControlPlugin::new(CommandExtensionConfig { name: "memo", dir: <CARGO_MANIFEST_DIR>/extensions/memo, main: "bootstrap.ts", commands: ["@memo"] })` — wires the `@memo` Node extension into the app.

  The root `Cargo.toml` depends on `bevy_cef` (path dep, `features = ["debug"]`) and on `ozmux_extension_host` with the `cef` feature enabled. A root `[features] debug` flag enables the CEF `remote-debugging-port` (a local Chromium DevTools / CDP endpoint on `127.0.0.1:9222`) for inspecting the embedded extension webview; it is off by default (`cargo run --features debug`).

- `crates/bevy_terminal` (`bevy_terminal`) — Bevy-native terminal: PTY ownership and `alacritty_terminal` VT emulation, emitting coalesced `FrameSnapshot` / `FrameDelta` against the `bevy_terminal_renderer` schema. Exposes `TerminalHandlePlugin`.
- `crates/bevy_terminal_renderer` (`bevy_terminal_renderer`) — GPU terminal renderer plus the grid schema shared with `bevy_terminal`. `TerminalRendererPlugin` wires the grid, material, and glyph sub-plugins (`TerminalGridPlugin`, `TerminalMaterialPlugin`, `TerminalGlyphPlugin`) and hyperlink-hover state; `schema` holds the cell/grid types both crates render against.
- `crates/extension_host` (`ozmux_extension_host`) — Tokio-free host for ozmux Node extensions: spawns an extension process, speaks a minimal length-prefixed byte protocol over its Unix socket, and (behind the `cef` feature) bridges its UI bytes through a `bevy_cef` `ozmux-ext://` custom scheme via the `bevy_cef_core` path dep. The `cef` feature is off by default so the core builds/tests with std + crossbeam only. Exposes `ExtensionControlPlugin` and `CommandExtensionConfig`.
- `crates/multiplexer` (`ozmux_multiplexer`) — ECS-native multiplexer. Session, Pane, and Surface are Bevy entities related by `ChildOf`; there are no typed IDs (every reference is a Bevy `Entity`, each carrying a `Name`). All mutations route through the `MultiplexerCommands` `SystemParam`; the only observers handle dangling `Entity` references when a child is despawned. Exposes `MultiplexerPlugin`.
- `crates/configs` (`ozmux_configs`) — config loader. Reads `~/.config/ozmux/config.toml` (or `$OZMUX_CONFIG` / `$XDG_CONFIG_HOME` overrides) and resolves it against built-in defaults.

In-process webview rendering is provided by the external `bevy_cef` crate (a path dependency on CEF v145, pinned to `145.6.1+145.0.28` in the Makefile). Both the renderer and the helper render process come from `bevy_cef` / `export-cef-dir`; see `make setup-cef`.

### TypeScript workspace (`pnpm-workspace.yaml`)

`packageManager` is `pnpm@10.30.2`. `catalogMode: strict` — shared versions for `@types/node`, `typescript`, `vitest`, `zod` live under `pnpm-workspace.yaml`'s `catalog:`. Workspace globs are `sdk/*` and `extensions/*`:

- `sdk/typescript` (`@ozmux/sdk`) — server-side SDK for extensions, with `./server`, `./cmd-shim`, and `./surface` exports; tests via `vitest`.
- `extensions/memo` (`memo`) — the `@memo` Node extension, consuming `@ozmux/sdk` via `workspace:*`.

### How the pieces connect at runtime

1. `ozmux-gui` boots a single Bevy `App`. `OzmuxBootstrapPlugin` seeds the initial session / pane / surface; `bevy_terminal` spawns the PTY and runs VT emulation, emitting frame snapshots/deltas that `bevy_terminal_renderer` draws on the GPU. Layout, input, copy-mode, IME, and shortcuts are all plugins in the same world.
2. `ozmux_extension_host`'s `ExtensionControlPlugin` launches the `@memo` extension as `node bootstrap.ts` (working dir `extensions/memo`), wiring it up over Unix sockets and (behind the `cef` feature) bridging its UI through `bevy_cef`'s `ozmux-ext://` scheme so the webview renders in-process. `node bootstrap.ts` runs the TypeScript entry directly, so it relies on a Node with native TypeScript type-stripping (Node ≥ 23.6).

### `src/` module map

`src/main.rs` plus: `bootstrap`, `clipboard`, `configs`, `extension_render`, `font`, `input`, `multiplexer`, `system_set`, `theme`, `ui`.

## Commands

### Rust

| Action                  | Command                                                                            |
| ----------------------- | ---------------------------------------------------------------------------------- |
| Build the workspace     | `cargo build` (or `make build`)                                                    |
| Run the app             | `cargo run` (or `make run`)                                                         |
| Run all tests           | `cargo test`                                                                        |
| Run one crate's tests   | `cargo test -p ozmux_multiplexer` (e.g. `cargo test -p ozmux_multiplexer <name>`)  |
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
| Build extension clients | `pnpm build` (required before `cargo run` for extensions with a Vite client, e.g. `md`) |

Biome (`biome.json`) scans `sdk/**` and `extensions/**` — it is the JS/TS lint+format tool for this repo.

## Other notable paths

- `.claude/rules/` — repo-wide Rust and TypeScript conventions (linked from the rules sections below).
- `docs/` — gitignored; safe place to drop specs/notes that should not be committed.

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
