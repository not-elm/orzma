# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Architecture

ozmux is a terminal multiplexer with a web UI. It is a hybrid Rust + TypeScript codebase organized as one Cargo workspace and one pnpm workspace sharing the same tree.

### Rust workspace (`Cargo.toml`)

Members live under `cli/` and `daemon/*`. Edition 2024, toolchain pinned to `1.95` (`rust-toolchain.toml`).

- `cli` (`ozmux`) — CLI dispatch via clap. Currently provides one subcommand:
  - `ozmux browser [URL]` — open an embedded browser activity in the current pane (reads `OZMUX_{WINDOW,PANE}_ID` from the PTY env).
- `daemon/bootstrap` (`daemon_bootstrap` binary) — process entry point. Sets up tracing, creates a per-PID runtime root under `$TMPDIR/ozmux/<pid>/{bin,sock}` (0700), loads extensions, wires `AppState`, and runs `ozmux_http_server::serve` until SIGINT.
- `daemon/browser` (`ozmux_browser`) — daemon-side client for the out-of-process CEF browser. `CefHostSupervisor` spawns the `cef_host` child and handshakes over a Unix-domain control socket; `CefHostHandles` exposes its command/event channels and an `is_dead` crash flag (a bootstrap watcher flips it and broadcasts `BrowserUnavailable` to WS subscribers when the child exits). `BrowserCefRegistry` holds, per Activity, a `FrameRing` plus a `NavState` watch channel. `FrameRing` (128 MiB / 60-frame budget) answers `subscribe()` with `FreshSnapshot` / `ResumeReplay` / `AwaitingKeyframe` / `MustRestart`. Frames are raw BGRA delivered through POSIX shared memory (`shm_alloc` / `shm_reader`); the shm fd is handed to `cef_host` per-`BrowserCreate` via `SCM_RIGHTS`. `cookie_extractor` seeds the embedded browser from the host Chrome cookie store via `decrypt-cookies` (macOS only). `OZMUX_BROWSER_SITE_ISOLATION=1` — restore Chromium's default Site Isolation in the embedded Browser Activity (off by default; local single-user tool, the trade-off is acceptable). `OZMUX_BROWSER_SKIP_COOKIE_IMPORT=1` — skip the host-Chrome cookie import; on macOS that import reads the `Chrome Safe Storage` Keychain item and raises an authorization dialog on every launch, so set this during local development (the embedded browser then opens unauthenticated). `make dev-daemon` sets it by default.
- `daemon/cef_host` (`ozmux_cef_host`, `cef_host` binary) — out-of-process CEF host built on `cef-rs`. Drives the Chromium message loop on the main thread and owns a `BrowserPool` of windowless (offscreen-rendered) CEF browsers, one per Browser Activity. A Tokio thread serves the UDS control plane: it receives `HostCommand`s (`BrowserCreate`, `Navigate`, `NavigateHistory`, `SendInput`, `Resize`, `Pause`/`ResumeScreencast`, …) and emits `HostEvent`s (`NavStateChanged`, …). `RenderHandler::on_paint` writes BGRA frames into the daemon-allocated shared-memory slots. On macOS the browser process launches with `--single-process --no-sandbox --disable-gpu --use-mock-keychain` (the last avoids a `Chromium Safe Storage` Keychain dialog).
- `daemon/http_server` (`ozmux_http_server`) — axum router on `127.0.0.1:3200`. `AppState` aggregates `MultiplexerState` (`Arc<Mutex<MultiplexerService>>`), `TerminalService`, and `ExtensionRegistry`, each wired in via `FromRef`. Top-level REST nests are `/sessions`, `/windows`, `/configs`; panes and activities are nested under windows (e.g. `/windows/{wid}/panes/{pid}/activities/{aid}/...`). Per-activity endpoints include a terminal WebSocket (`/.../terminal/ws`, msgpack `WireMessage` frames), a browser WebSocket (`/.../browser/ws`, msgpack `BrowserServerMsg`/`BrowserClientMsg` frames, BGRA screencast), an extension handlers WebSocket (`/.../handlers/ws`), and an iframe passthrough (`/.../iframe/{*path}`). `serve` bootstraps one session/window/pane/activity and spawns its PTY before the listener binds. The embedded `index.html` lives at `src/handlers/index.html`, where `vite build` writes it.
- `daemon/multiplexer` (`ozmux_multiplexer`) — pure in-memory domain model. `MultiplexerService` owns five stores (`SessionState`, `WindowState`, `PaneState`, `LayoutCellState`, `ActivityState`) plus a `pane_to_cell` index. The Session → Window → Pane → Activity hierarchy is layered with a separate cell tree (`Cell::Root` / `Cell::Pane` / split nodes) that drives layout; mutations like `split_pane` and `close_pane` keep the indices and `active_pane` consistent transactionally. No I/O — terminal lifecycle is delegated.
- `daemon/terminal` (`ozmux_terminal`) — PTY service + server-side VT emulator. `TerminalService::spawn(pane, activity, SpawnOptions)` launches a `portable-pty` child and a per-activity bridge task that feeds PTY bytes into `alacritty_terminal::Term` inside `VtState` (`src/vt/bridge.rs`). `VtState` produces snapshot/delta frames via `frame_builder.rs`, encodes them as msgpack `WireMessage`s, and fans them out on a `broadcast::Sender<WireMessage>`. WS clients call `subscribe_frames()` and receive a `FrameSubscription` (`FreshSnapshot` or `ResumeReplay` with backfilled deltas from the in-memory `FrameRing`). Raw `TerminalEvent`s are internal — the WS does not see PTY bytes directly. msgpack wire format is contract-tested against fixtures under `tests/fixtures/wire_msgpack/` (see `make test-wire-*`).
- `daemon/extension` (`ozmux_extension`) — extension host. `RuntimeRoot::resolve_in` picks a parent directory whose resulting UDS sun_path fits the platform limit (104 macOS / 108 Linux), falling back to `/tmp`. `ExtensionHandles::load` discovers Node extensions under `$OZMUX_EXTENSION_ROOT` and registers them in `ExtensionRegistry`. `bootstrap::longest_extension_name` is used to size the sock_dir conservatively at startup.
- `daemon/configs` (`ozmux_configs`) — config loader. Reads `~/.config/ozmux/config.toml` (or `$OZMUX_CONFIG` / `$XDG_CONFIG_HOME` overrides), merges onto built-in defaults, and exposes `shortcuts` + `theme` submodules. Returns `Default::default()` when no file is present.
- `daemon/macros` (`ozmux_macros`) — proc-macro crate (syn/quote/darling); compile-fail tests use `trybuild`.

### TypeScript workspace (`pnpm-workspace.yaml`)

`packageManager` is `pnpm@10.30.2`. `catalogMode: strict` — shared versions for `@playwright/test`, `@types/node`, `typescript`, `vitest`, `zod`, etc. live under `pnpm-workspace.yaml`'s `catalog:`. Workspace globs:

- `daemon/frontend` (`ozmux-ui`) — Vite 8 + React 19 (with React Compiler) + Tailwind v4. The terminal is a **custom React DOM renderer** (no xterm.js): `src/terminal/` decodes msgpack `WireMessage` frames over WebSocket (`msgpackr`), maintains a grid store, and renders each visible cell as a Tailwind-styled `<span>` (`renderer/TerminalGrid.tsx`, `renderer/Row.tsx`). Grapheme widths use `string-width`; font metrics are probed from a `font-mono` element (`renderer/font.ts`). Cursor and IME live in DOM overlays (`overlay/`); mouse/keyboard input is handled in `input/`. Browser Activities (`src/browser/`) are rendered by a toolbar plus a `<canvas>` whose control is transferred to a Web Worker; the worker decodes msgpack `BrowserServerMsg` frames and paints the BGRA screencast via a WebGPU or WebGL2 renderer (`renderer/factory.ts` picks per platform), with keyboard/mouse/wheel/IME input forwarded back over the same socket. Built with `vite-plugin-singlefile` so `vite build` produces one self-contained `index.html`, written to `daemon/http_server/src/handlers/index.html` and embedded into the Rust binary. The Makefile's `verify-out-dir` target fails the build if anything other than `*.rs` and `index.html` shows up alongside it — the inliner is supposed to leave no sidecars.
- `sdk/*` — TypeScript SDKs. Currently `sdk/typescript` (`@ozmux/sdk`): server-side SDK for extensions with `./server` and `./cmd-shim` exports; tests via `vitest`.
- `extensions/*` — Node extensions. Currently `extensions/memo`, consuming `@ozmux/sdk` via `workspace:*`. Extensions are discovered at daemon startup via `OZMUX_EXTENSION_ROOT`.
- `daemon/extension/tests/fixtures/*` — fixture packages for the Rust extension host's integration tests.

### How the pieces connect at runtime

1. `daemon_bootstrap` reads `OZMUX_EXTENSION_ROOT`, creates the runtime root, spawns extension Node processes (UDS in `sock/`), and starts the axum server.
2. The browser loads the embedded `index.html` from `GET /`. In debug builds, `/` redirects to `http://localhost:5173` so Vite HMR can be used.
3. The frontend opens a WebSocket to `/windows/{wid}/panes/{pid}/activities/{aid}/terminal/ws` for the bootstrap activity. The daemon does server-side VT emulation (`alacritty_terminal::Term` inside `VtState`) and broadcasts msgpack `WireMessage` frames (snapshot + delta); the frontend's custom DOM renderer applies them to its grid store. Keyboard, mouse, and resize messages travel back over the same socket.
4. Extensions are reachable via `/windows/{wid}/panes/{pid}/activities/{aid}/iframe/*` (proxied to the extension's HTTP server over its UDS).

## Commands

### Rust

| Action | Command |
| --- | --- |
| Build everything | `cargo build` |
| Build the daemon binary | `cargo build -p daemon_bootstrap` |
| Run the daemon (with extensions) | `make dev-daemon` (sets `OZMUX_EXTENSION_ROOT=$PWD/extensions`) |
| Run a single test | `cargo test -p ozmux_multiplexer close_pane_after_split_fully_reverts_state` |
| Run one crate's tests | `cargo test -p ozmux_http_server` |
| Lint + format (Rust) | `cargo clippy --fix --allow-dirty --allow-staged && cargo fmt` |
| Fix everything | `make fix-lint` (runs clippy fix, rustfmt, and `pnpm lint:fix`) |
| Terminal wire-protocol golden tests | `make test-wire-goldens` (diff `*.diag.txt` fixtures) |
| Regenerate + verify msgpack fixtures | `make test-wire-contract` (uses `tools/verify-msgpack.ts`) |

Logs go through `tracing-subscriber`. Default filter is `info,hyper=warn,tower=warn,tokio_tungstenite=warn,tungstenite=warn`; override with `RUST_LOG`.

### TypeScript / frontend

| Action | Command |
| --- | --- |
| Install workspace deps | `pnpm install --frozen-lockfile` |
| Vite dev server on `:5173` (HMR) | `pnpm dev` or `make dev-frontend` |
| Typecheck every package | `pnpm check-types` |
| Run all vitest suites | `pnpm test` |
| Run one SDK test file | `pnpm --filter @ozmux/sdk exec vitest run path/to/file.test.ts` |
| Lint (biome) | `pnpm lint` / `pnpm lint:fix` / `pnpm lint:ci` |

Biome (`biome.json`) only scans `daemon/frontend/**` — it is the JS/TS/CSS lint+format tool for this repo, configured for 2-space indent, single quotes, 100-col width, and Tailwind directives in CSS. Custom GritQL plugins under `biome-plugins/` enforce the styling rules (no inline styles, no arbitrary Tailwind values, no raw `--tn-*` palette refs).

## Other notable paths

- `tools/` — wire-protocol diagnostic helpers (`bin-to-diag.sh`, `msgpack-to-diag.sh`, `verify-msgpack.ts`). Used by `make test-wire-*`.
- `scripts/dev-e2e.sh` — lifecycle script behind the `make dev-e2e*` targets.
- `.claude/rules/` — repo-wide Rust and styling conventions (linked from the rules sections below).
- `.ozmux/` — runtime state from the e2e harness (PID file, logs); gitignored.
- `docs/` — gitignored; safe place to drop specs/notes that should not be committed.

## Comment language

All in-code comments — line comments (`//`), doc comments (`///`, `//!`),
and block comments in any language under this repo — must be written in
English. This applies to Rust (`cli/`, `daemon/*`), TypeScript/React
(`daemon/frontend`, `sdk/*`, `extensions/*`), CSS, shell scripts, and
config files. Use English even when the conversation with the user is in
another language. Identifiers and string literals are not constrained by
this rule; only comments are.

## Styling

Frontend styling (utility-first Tailwind v4, semantic tokens, no inline
styles, no arbitrary values, no raw palette references) is governed by
[`.claude/rules/styling.md`](.claude/rules/styling.md) and enforced by
Biome GritQL plugins in `biome-plugins/`.

## Rust Coding Rules

Rust style and conventions (no `mod.rs`, restricted comment taxonomy,
doc-comment policy, import discipline) are governed by
[`.claude/rules/rust.md`](.claude/rules/rust.md). Applies to all crates
under `cli/` and `daemon/*`.

## UI verification workflow

Use this when you have changed anything under `daemon/frontend/src/**`, the showcase, theme tokens, pane layout, or daemon-side endpoints that the UI consumes. Skip it for purely backend-internal changes that the UI does not exercise.

### First-time setup (per checkout)

1. Run prerequisites once:

   ```bash
   make dev-e2e-setup
   ```

   This installs JS dependencies, warms the Rust build cache, and downloads the Playwright Chromium binary.

2. In Claude Code, approve the project-scoped Playwright MCP server once:

   ```
   /mcp
   ```

   Approve the `playwright` server. The pinned version is `@playwright/mcp@0.0.75` with `--isolated --headless`.

### Verification loop

1. Start the harness in the background:

   ```bash
   make dev-e2e
   ```

   Wait for the single `ready` line on stdout. If it errors with "port already in use", inspect with `lsof -nP -iTCP:<port> -sTCP:LISTEN` and free the port before retrying.

2. Drive the browser via the Playwright MCP tools. Navigate to `http://localhost:5173`. Use `browser_snapshot` for DOM inspection, `browser_take_screenshot` for visual checks, and `browser_console_messages` to read errors.

3. When done, stop everything:

   ```bash
   make dev-e2e-stop
   ```

### Failure modes

| Symptom | Cause | Recovery |
| --- | --- | --- |
| `error: port 5173 is already in use.` | Stray Vite or another process | `lsof -nP -iTCP:5173 -sTCP:LISTEN`, kill the holder |
| `error: port 3200 is already in use.` | Stray daemon | same, for port 3200 |
| `error: harness already running (see .ozmux/e2e.pid).` | A previous harness is still up | `make dev-e2e-stop` |
| `error: readiness timeout after 30s.` | Vite or daemon failed to come up | Read the last 20 lines printed from `.ozmux/logs/vite.log` and `.ozmux/logs/daemon.log` |
| MCP tools missing or fail | Server not approved | Run `/mcp` and approve `playwright` |

### What lives where

- `scripts/dev-e2e.sh` — lifecycle script (start/stop/setup).
- `Makefile` — `dev-e2e`, `dev-e2e-setup`, `dev-e2e-stop` targets dispatch to the script.
- `.mcp.json` — registers `@playwright/mcp` (pinned).
- `.ozmux/` — runtime state (PID file, logs); gitignored.
