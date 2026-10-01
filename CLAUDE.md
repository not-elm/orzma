# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Architecture

orzma is a terminal that runs as a single native GUI application; a built-in multiplexer (panes and tabs) runs in the same process. It is a hybrid Rust + TypeScript codebase organized as one Cargo workspace and one pnpm workspace sharing the same tree. There is no daemon, no HTTP server, and no browser-side frontend — terminal emulation, GPU rendering, layout, input, and webview rendering all run in one Bevy ECS world.

### Rust workspace (`Cargo.toml`)

The workspace root package is the one and only binary; library crates live under `crates/`. Edition 2024, toolchain pinned to `1.95` (`rust-toolchain.toml`).

- `orzma` (workspace root, `src/main.rs`) — the single binary: a Bevy 0.19 app. `main()` builds one `App` and adds `DefaultPlugins` (configured with a `WindowPlugin` titled "orzma" and a `RenderPlugin` that compiles pipelines synchronously) plus `cef_plugin(orzma_registry.clone(), cef_profile.path())` (from `bevy_cef`), then the orzma plugins:
  - `SurfacePlugin`, `SessionPlugin`, `OrzmuxPlugin` (from `bevy_orzmux`), `TerminalRendererPlugin` (from `bevy_orzma_tty_renderer`), `ActionPlugin`, `OrzmaConfigsPlugin`, `FontBridgePlugin`, `OrzmaInputPlugin` (`input`'s root plugin, aggregating `ShortcutsPlugin`, `OptionAsAltPlugin`, `KeyboardInputPlugin`, `MouseInputPlugin`, `FocusSyncPlugin`, `ImePlugin`, and `HyperlinkInputPlugin`), `OrzmaUiPlugin` (`ui`'s root plugin, aggregating the UI root, the shell-surface subtree, the tab bar, the IME overlay, and the vi-mode indicator);
  - `OrzmaWebviewPlugin` (from `bevy_orzma_webview`), `WindowTitlePlugin`, `WindowIconPlugin` (Windows-only: applies the icon `build.rs` embeds in the executable from `build/windows/orzma.ico` to the title bar and taskbar), `WindowMonitorPlugin` (keeps the window alive when the monitor it is on disappears, e.g. on display sleep; Bevy 0.19.0 and 0.19.1 otherwise despawn the window with the monitor), `RedrawPlugin` (`redraw`'s root plugin: on-demand updates, follow-up frames, the caret-blink wake timer, and the webview update tick).
  - The GUI half of the in-process webview feature — CEF render wiring, the `window.orzma` page bridge, and the webview entities — is aggregated under `OrzmaWebviewPlugin` (from `crates/bevy_orzma_webview`). The control-socket listener and every client's state live in `orzma_webview_host`, which the `orzmux` thread runs.

  The root `Cargo.toml` depends on `bevy_orzma_webview` (path dep) and on `bevy_cef` (crates.io, `0.13`). A root `[features] debug` flag (forwarded through `bevy_orzma_webview/debug` to `bevy_cef/debug`) enables the CEF `remote-debugging-port` (a local Chromium DevTools / CDP endpoint on `127.0.0.1:9222`) for inspecting the embedded webview; it is on by default (`default = ["debug"]`), so `cargo run` exposes the port, and the release packaging scripts build with `--no-default-features` to leave it out.

- `crates/orzma_vt` (`orzma_vt`) — VT emulation: the `Vt` trait, `OrzmaVt`'s screen/grid/viewport model, CSI/OSC/APC dispatch, selection, vi-cursor state, and the palette/color types. No Bevy or PTY dependency; consumed by `orzma_tty`, `orzmux`, `bevy_orzmux`, and `bevy_orzma_tty_renderer`.
- `crates/orzma_tty` (`orzma_tty`) — PTY-backed terminal core: spawns the login shell under a PTY (or, for tests, a PTY-less fake master) and drives an injected `Vt` implementor behind a frame coalescer. Exposes `OrzmaTty`, `SpawnOptions`, the key / mouse / paste input encoders, and the wheel and pointer routers; no Bevy dependency.
- `crates/orzmux` (`orzmux`) — the built-in multiplexer backend: a Bevy-free thread that owns every pane's PTY-backed `OrzmaTty<OrzmaVt>` and one cell-unit split tree per tab (`backend/layout`, `backend/tab`), waits on the command channel, the control socket's events, and each pane's PTY streams with one `crossbeam_channel::Select`, and emits `OrzmuxEvent`s (`Layout` / `Frame` / `Signal` / `PaneOpened` / `SpawnFailed` / `PaneClosed` / `SelectionText` / `SelectionCopied` / `Webview` / `Tabs`, defined in `backend`) to the GUI. The crate splits into a business-logic `Backend` (`backend`, owning the panes and the layout tree) and a transport-layer `EventLoop` (`event_loop`, driving the `Backend` over the channels) that depends on it one way. The `Backend` owns a `WebviewHost<PaneId>` and calls it at pane spawn (which hands the shell `ORZMA_SOCK` and a fresh `ORZMA_TOKEN`), for each VT placement signal, at pane close, and on each active-pane change; the host's events reach the GUI as `OrzmuxEvent::Webview` and its requests are applied on the spot. The GUI holds an `OrzmuxClient` and sends `OrzmuxCommand`s (`Resize`, `NewPane`, `KillPane`, `SelectPane*`, key / pointer / wheel / paste / scroll / selection input, `CloseTab` / `SelectTab` / `RenameTab` / `MoveTab`, and `Webview` reports; defined in `event_loop`). Every type crossing the channel is plain data so the transport can later become a socket to a separate process.
- `crates/bevy_orzmux` (`bevy_orzmux`) — Bevy integration for `orzmux`: the `OrzmuxConnection` resource, the `CurrentTabs` resource, the `OrzmuxPane(PaneId)` component each pane entity carries (with `TtyTitle` and, via the renderer, `TerminalView` and `TerminalCells` as required components), `drain_orzmux_events` (turns backend events into `Tty*Signal` `EntityEvent`s, entity lifecycle, and one `OrzmuxWebviewEvent` trigger per webview event, with a mount's pane resolved to its entity), `apply_layout` (absolute pane nodes and separators from the latest `Layout`, hiding panes absent from the layout), and the inbound `RequestTty*` / `RequestActive*` / `RequestPaneAction` / `RequestTabAction` observers that send `OrzmuxCommand`s. Exposes `OrzmuxPlugin`.
- `crates/bevy_orzma_tty_renderer` (`bevy_orzma_tty_renderer`) — GPU terminal renderer. Its feature modules (`grid`, `hyperlink`, `cursor`, `font`, `glyph`, `material`, and the shared `system_set` and `pane_style`) are private; downstream crates reach it through `prelude` and the `bundled` font bytes. `TerminalRendererPlugin` wires the grid, material, glyph, font, and cursor sub-plugins and the hyperlink-hover state. The `grid` module mirrors each pane's `TtyFrameSignal` frames into `TerminalView` (viewport geometry, cursor, selection, placements) and `TerminalCells` (`orzma_vt` cells, palette, hyperlink table), split so a cursor or selection change never re-uploads the GPU cell buffer.
- `crates/bevy_orzma_webview` (`bevy_orzma_webview`) — the GUI half of the webview feature: depends on `orzma_vt`, `orzmux`, `bevy_orzmux`, `bevy_orzma_tty_renderer`, `orzma_webview_host`, and `bevy_cef`. It holds no client state: it spawns, resizes, and despawns webview entities as the host's `OrzmuxWebviewEvent`s arrive, keyed by the host's `MountId`; serves registered assets through the `orzma://` custom scheme from a copy of the host's asset table; relays the `window.orzma` page bridge, address changes, and first compositing to the host as `WebviewCommand`s; mirrors the host's webview focus into `FocusedWebview`, which only its focus module writes, fed by `RequestWebviewFocus`; and runs the CPU paint bridge that copies CEF's `on_paint` frames into each webview's headless `WebviewTextureTarget` on Windows and Linux (`bevy_cef` writes that target only on macOS). Exposes `OrzmaWebviewPlugin`, `cef_plugin`, `RequestWebviewFocus`, and `WebviewAssetRegistry`.
- `crates/orzma_webview_host` (`orzma_webview_host`) — the server side of the webview feature, free of Bevy and CEF: the control-socket listener (accept loop, per-connection reader and writer threads, peer checks) with its NDJSON wire types, and `WebviewHost<P>`, which owns every client's state (connections, tokens, registrations, instances, mounts, focus, in-flight page calls, compositing) and every decision about it. Its entry points return plain-data `WebviewEvent`s for the GUI and `MuxRequest`s for the multiplexer; `orzmux` runs it as `WebviewHost<PaneId>`. It also holds the `RuntimeRoot` socket directory tree and the boundary types (`MountId`, `MountSpec`, `WebviewCommand`, `ForwardChord`, `HandleId`, `WebviewAsset`), and gathers its failures in `WebviewHostError`.
- `crates/orzma_configs` (`orzma_configs`) — config loader. Reads `~/.config/orzma/config.toml` (or `$ORZMA_CONFIG` / `$XDG_CONFIG_HOME` overrides) and resolves it against built-in defaults.
- **Platforms.** macOS is the primary platform. Windows 10 1809+ / 11 (x64) is
  supported for the `orzma` binary, the `ratatui_orzma` SDK, and the companion
  apps (`apps/orzmd`, `apps/orzbrowser`); the SDK reaches the control socket
  through `uds_windows` there and mounts webviews with the socket `mount` op,
  since ConPTY drops the APC verb. Linux (x86_64, glibc 2.35+) is supported
  for the `orzma` binary and the companion apps through the tar.gz and the
  `.deb` built by `release-linux.yml` (`just bundle` on Linux).

In-process webview rendering is provided by the external `bevy_cef` crate (crates.io `0.13`, CEF v152 pinned to `152.4.0+152.0.8` in the justfile). Both the renderer and the helper render process come from `bevy_cef` / `export-cef-dir`; see `just setup-cef`.

### TypeScript workspace (`pnpm-workspace.yaml`)

`packageManager` is `pnpm@10.30.2`. `catalogMode: strict` — shared versions for `@types/node`, `typescript`, `vitest` live under `pnpm-workspace.yaml`'s `catalog:`. Workspace packages are `sdk/*`:

- `sdk/orzma-web` (`@orzma/web`) — in-page TypeScript client for the `window.orzma` bridge (`orzma`, `isOrzmaAvailable`, `OrzmaApi`); tests via `vitest`.
- `sdk/orzma-scroller` (`@orzma/scroller`) — Vimium-style keyboard scrolling (`installScroller`, `HeldKeys`) shared by the orzbrowser and orzmd pages; private and consumed as TypeScript source; tests via `vitest`.

### How the pieces connect at runtime

1. `orzma` boots a single Bevy `App`, starts the `orzmux` backend thread with `OrzmuxClient::spawn`, sends the window geometry (`OrzmuxCommand::Resize`) once font metrics exist, and requests the first pane (`PaneSpawnRequest { Tab }`). The backend spawns the login shell under a PTY driving an `orzma_vt::OrzmaVt`, and streams `Layout` snapshots plus coalesced `Frame`s that `bevy_orzma_tty_renderer` draws on the GPU. Splits, directional selection, and kills are `OrzmuxCommand`s; the backend owns the active pane and the GUI mirrors it into `KeyboardFocused`. The app runs on demand (`src/redraw`): it updates only when woken — by the orzmux backend right after it queues work (control-socket traffic reaches the GUI through it), through a `std::task::Waker` built from winit's `EventLoopProxy`, by a timer thread at the next caret blink, or by window input — and ticks at about 30 Hz while a webview exists; `Time<Real>` reads the system clock at `First`. A Bevy upgrade must re-verify that the winit runner still re-reads `Messages<RequestRedraw>` after every update, that winit's `RedrawRequested` still counts as a window event for bevy_winit's update decision without being forwarded into `Messages<WindowEvent>`, that `Time<Real>` still falls back to the system clock when `TimeReceiver` is absent, whether it includes bevyengine/bevy#25427 (0.19.2 / 0.20), which makes `WindowMonitorPlugin` removable, and whether it includes bevyengine/bevy#24845 (0.20), which makes `ModifierSyncPlugin` (`src/input/keyboard/modifier_sync.rs`) removable; that change synthesizes `KeyboardInput` releases on every platform, so re-check that the modifier-tap leader does not fire from them and that AltGr still works on Windows.
2. A program registers webview content over the control socket, which the webview host (`orzma_webview_host`) serves on the `orzmux` thread, and gets an opaque handle and its first placement instance. It then writes an APC `Omount;n=<instance>` sequence: the pane's VT reports the placement to the host, which checks that the pane owns the instance and tells the GUI to mount an in-process `bevy_cef` webview (assets served from disk/memory via `orzma://`, one origin per handle). Where the PTY drops APC (ConPTY on Windows), the program sends the equivalent `mount` / `unmount` ops over the control socket instead. Additional placements of the same handle are minted with `new_instance` over the same socket. The page talks back to the registering program through `window.orzma.call/on`, which the GUI relays to the host and the host to the program over the control socket.

### `src/` module map

`src/main.rs` plus: `action`, `cef_profile`, `configs`, `font`, `input`, `redraw`, `session`, `surface`, `system_set`, `ui`, `window_icon`, `window_monitor`, `window_title`.

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
| Provision CEF (one-time) | `just setup-cef` (installs the CEF framework + render process; macOS, Windows, and Linux) |

Logs go through `tracing-subscriber`; override the filter with `RUST_LOG`.

### TypeScript

| Action                  | Command                                                |
| ----------------------- | ------------------------------------------------------ |
| Install workspace deps  | `pnpm install`                                         |
| Run all vitest suites   | `pnpm -r test`                                         |
| Typecheck every package | `pnpm check-types`                                     |
| Lint (biome)            | `pnpm lint` / `pnpm lint:fix` / `pnpm lint:ci`         |

Biome (`biome.json`) scans `sdk/**` — it is the JS/TS lint+format tool for this repo.

### Documentation

| Action                                    | Command           |
| ----------------------------------------- | ----------------- |
| Install the pinned mdBook tools (one-time) | `just setup-book` |
| Build the user guide into `target/book`    | `just book`       |
| Preview the user guide with live reload    | `just book-serve` |

## Other notable paths

- `.claude/rules/` — repo-wide Rust and TypeScript conventions (linked from the rules sections below).
- `docs/book/` — the user guide (mdBook), published to GitHub Pages when a release is published. A change to user-visible behavior (configuration keys, shortcuts, the webview protocol) updates the matching page under `docs/book/src/` in the same change.
- `docs/memo/`, `docs/references/`, `docs/todo/` — implementation notes, reference manuals, and per-task working notes.
- `CONTRIBUTING.md` — contributor guide: development setup, architecture, conventions, pull requests, and documentation.

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
doc-comment policy, import discipline, `Result`-based error handling
instead of asserts and unwraps) are governed by
[`.claude/rules/rust.md`](.claude/rules/rust.md). Applies to the root
binary (`src/`) and all crates under `crates/`.

## TypeScript Coding Rules

TypeScript style and conventions (restricted comment taxonomy, JSDoc on
exports, export-visibility minimization, justified suppressions) are
governed by [`.claude/rules/typescript.md`](.claude/rules/typescript.md).
Applies to `sdk/*` and `extensions/*`.
